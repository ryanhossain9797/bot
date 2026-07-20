
use crate::handle::machines;
use crate::store::store;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const BATCH: i64 = 50;
const MAX_PAGES: i64 = 20;
const OUTBOX_GRACE_MS: i64 = 60_000;
const MIN_INTERVAL: Duration = Duration::from_secs(10);
const MAX_INTERVAL: Duration = Duration::from_secs(30);
const LOOK_AHEAD_MARGIN: Duration = Duration::from_secs(30);
const TIMER_LOOK_AHEAD_MS: i64 = (MAX_INTERVAL.as_secs() + LOOK_AHEAD_MARGIN.as_secs()) as i64 * 1000;
const REWAKE_SUPPRESS: Duration = Duration::from_secs(60);
const WAKE_STAGGER: Duration = Duration::from_millis(25);

pub fn start() {
    tokio::spawn(sweeper_loop());
}

async fn sweeper_loop() {
    let mut recently_woken: HashMap<(String, String), Instant> = HashMap::new();
    sweep_once((0, TIMER_LOOK_AHEAD_MS), &mut recently_woken).await;
    let mut interval = MIN_INTERVAL;
    loop {
        let woke = sweep_once((OUTBOX_GRACE_MS, TIMER_LOOK_AHEAD_MS), &mut recently_woken).await;
        interval = if woke == 0 { (interval * 2).min(MAX_INTERVAL) } else { MIN_INTERVAL };
        tokio::time::sleep(interval).await;
    }
}

async fn paged<F, Fut>(what: &str, query: F) -> Vec<(String, String)>
where
    F: Fn(i64) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<(String, String)>>>,
{
    let mut all = Vec::new();
    for page in 0..MAX_PAGES {
        match query(page * BATCH).await {
            Ok(rows) => {
                let last = (rows.len() as i64) < BATCH;
                all.extend(rows);
                if last {
                    return all;
                }
            }
            Err(e) => {
                println!("[sweep] {what} query failed (next pass retries): {e:#}");
                return all;
            }
        }
    }
    println!(
        "[sweep] {what} candidates clipped at {}; remainder picked up on later passes",
        BATCH * MAX_PAGES
    );
    all
}

async fn sweep_once(
    (outbox_grace_ms, timer_look_ahead_ms): (i64, i64),
    recently_woken: &mut HashMap<(String, String), Instant>,
) -> usize {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let stalled = paged("stalled-outbox", |offset| {
        store().stalled_outbox_senders(now_ms - outbox_grace_ms, BATCH, offset)
    })
    .await;
    let due = paged("coming-due-timers", |offset| {
        store().due_timers(now_ms + timer_look_ahead_ms, BATCH, offset)
    })
    .await;

    let now = Instant::now();
    recently_woken.retain(|_, woken_at| now.duration_since(*woken_at) < REWAKE_SUPPRESS);

    let mut woke = 0usize;
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for (machine, id_json) in stalled.into_iter().chain(due) {
        if woke >= BATCH as usize {
            break;
        }
        let key = (machine, id_json);
        if !seen.insert(key.clone()) || recently_woken.contains_key(&key) {
            continue;
        }
        let (machine, id_json) = &key;
        match machines().get(machine) {
            Some(vtable) => {
                (vtable.wake)(id_json.clone()).await;
                recently_woken.insert(key, now);
                woke += 1;
                tokio::time::sleep(WAKE_STAGGER).await;
            }
            None => println!("[sweep] no machine named {machine} registered; skipping {id_json}"),
        }
    }
    if woke > 0 {
        println!("[sweep] woke {woke} entities with stalled outbox rows or coming-due timers");
    }
    woke
}
