//! Turso-backed persistence (#186 pass 1). The store — not the in-memory registry — is the
//! transaction guarantee: state CAS, outbox inserts, and the receiver dedup upsert commit
//! atomically. `init_turso_store` must run before any state machine is used.

use anyhow::Context;
use std::sync::OnceLock;

/// What a durable outbox row does at the target: deliver an action, or construct the entity
/// (idempotent — an existing target makes it a no-op). Serial per-sender dispatch means
/// Construct-then-Act rows compose into subject's ActMaybeConstruct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowKind {
    Act,
    Construct,
}

impl RowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RowKind::Act => "act",
            RowKind::Construct => "construct",
        }
    }

    pub fn parse(s: &str) -> Option<RowKind> {
        match s {
            "act" => Some(RowKind::Act),
            "construct" => Some(RowKind::Construct),
            _ => None,
        }
    }
}

/// An internal entity→entity operation captured during a transition, already serialized —
/// first-class data, unlike external effects which stay opaque futures.
#[derive(Debug, Clone)]
pub(crate) struct OutboxDraft {
    pub kind: RowKind,
    pub target_machine: &'static str,
    pub target_id_json: String,
    pub payload_json: String,
}

/// A durable outbox row loaded back from the store (drain-on-activate / boot recovery).
#[derive(Debug, Clone)]
pub(crate) struct OutboxRow {
    pub seq: i64,
    pub kind: RowKind,
    pub target_machine: String,
    pub target_id_json: String,
    pub payload_json: String,
}

/// Identifies one delivered action for receiver-side dedup: the sender plus the
/// outbox row's per-sender sequence number. Costs nothing extra — it's the row's identity.
#[derive(Debug, Clone)]
pub(crate) struct CallToken {
    pub sender_machine: &'static str,
    pub sender_id: String,
    pub seq: i64,
}

/// Everything one committed transition writes, atomically.
pub(crate) struct TransitionWrite {
    pub machine: &'static str,
    pub id_string: String,
    pub state_json: String,
    pub expected_version: i64,
    pub first_seq: i64,
    pub next_outbox_seq: i64,
    /// Deadline derived from `SM::schedule(next_state)` at commit time — the sweep reads this
    /// column instead of deserializing state blobs.
    pub next_tick_on: Option<i64>,
    pub outbox: Vec<OutboxDraft>,
    pub dedup: Option<CallToken>,
}

pub(crate) enum SaveOutcome {
    Ok,
    /// CAS miss: the store moved under us (or the row is gone). Policy: reload-and-drop.
    Conflict { actual: Option<i64> },
}

pub(crate) struct LoadedEntity {
    pub state_json: String,
    pub version: i64,
    pub next_outbox_seq: i64,
}

pub(crate) struct Store {
    db: turso::Database,
}

static STORE: OnceLock<Store> = OnceLock::new();

/// Point the framework at a Turso database file. Must be called before any state machine
/// is used; entities/outbox/call_dedup tables are created if missing.
pub async fn init_turso_store(path: &str) -> anyhow::Result<()> {
    if let Some(dir) = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("create_dir_all {}", dir.display()))?;
    }
    let db = turso::Builder::new_local(path)
        .build()
        .await
        .with_context(|| format!("open turso db at {path}"))?;
    create_tables(&db).await?;
    STORE
        .set(Store { db })
        .map_err(|_| anyhow::anyhow!("store already initialized"))
}

async fn create_tables(db: &turso::Database) -> anyhow::Result<()> {
    let conn = db.connect().context("connect for schema init")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entities (
             machine TEXT NOT NULL,
             id TEXT NOT NULL,
             id_json TEXT NOT NULL,
             state TEXT NOT NULL,
             version INTEGER NOT NULL,
             next_outbox_seq INTEGER NOT NULL,
             next_tick_on INTEGER,
             PRIMARY KEY (machine, id)
         );
         CREATE TABLE IF NOT EXISTS outbox (
             sender_machine TEXT NOT NULL,
             sender_id TEXT NOT NULL,
             seq INTEGER NOT NULL,
             sender_id_json TEXT NOT NULL,
             target_machine TEXT NOT NULL,
             target_id_json TEXT NOT NULL,
             action TEXT NOT NULL,
             kind TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             failure TEXT,
             PRIMARY KEY (sender_machine, sender_id, seq)
         );
         CREATE TABLE IF NOT EXISTS call_dedup (
             machine TEXT NOT NULL,
             id TEXT NOT NULL,
             caller_machine TEXT NOT NULL,
             caller_id TEXT NOT NULL,
             last_seq INTEGER NOT NULL,
             PRIMARY KEY (machine, id, caller_machine, caller_id)
         );",
    )
    .await
    .context("create framework tables")
}

pub(crate) fn store() -> &'static Store {
    STORE
        .get()
        .expect("store not initialized — call re_framework::init_turso_store before using state machines")
}

impl Store {
    fn connect(&self) -> anyhow::Result<turso::Connection> {
        let conn = self.db.connect().context("turso connect")?;
        // concurrent actors write from separate connections; wait for the lock instead of failing busy
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("set busy_timeout")?;
        Ok(conn)
    }

    pub async fn load(&self, machine: &'static str, id_string: &str) -> anyhow::Result<Option<LoadedEntity>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT state, version, next_outbox_seq FROM entities WHERE machine = ? AND id = ?",
                (machine, id_string),
            )
            .await
            .context("load entity")?;
        match rows.next().await.context("load entity row")? {
            None => Ok(None),
            Some(row) => Ok(Some(LoadedEntity {
                state_json: row.get(0).context("state column")?,
                version: row.get(1).context("version column")?,
                next_outbox_seq: row.get(2).context("next_outbox_seq column")?,
            })),
        }
    }

    /// Construct-time insert. `Conflict` means the row already exists (lost a construct race).
    pub async fn insert(
        &self,
        machine: &'static str,
        id_string: &str,
        id_json: &str,
        state_json: &str,
        next_tick_on: Option<i64>,
        outbox: &[OutboxDraft],
    ) -> anyhow::Result<SaveOutcome> {
        let conn = self.connect()?;
        conn.execute("BEGIN IMMEDIATE", ()).await.context("begin insert")?;
        let result = insert_in_tx(&conn, machine, id_string, id_json, state_json, next_tick_on, outbox).await;
        finish_tx(&conn, result).await
    }

    /// The atomic transition unit: {state CAS + outbox inserts + dedup upsert}, commit-or-nothing.
    pub async fn save(&self, write: &TransitionWrite, id_json: &str) -> anyhow::Result<SaveOutcome> {
        let conn = self.connect()?;
        conn.execute("BEGIN IMMEDIATE", ()).await.context("begin save")?;
        let result = save_in_tx(&conn, write, id_json).await;
        finish_tx(&conn, result).await
    }

    /// Has this call already been applied? (Receiver-side check before transitioning.)
    /// Monotonic: any seq at or below the slot is a duplicate. Sound because a dispatcher
    /// never advances past a row without a definitive Applied/Duplicate/Rejected verdict,
    /// so every seq below the slot was already resolved — and it makes redelivery of an
    /// older row (ack that gave up, orphaned dispatcher racing its successor) a no-op
    /// instead of a double-apply, which strict equality could not guarantee.
    pub async fn is_duplicate(
        &self,
        machine: &'static str,
        id_string: &str,
        token: &CallToken,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT last_seq FROM call_dedup
                 WHERE machine = ? AND id = ? AND caller_machine = ? AND caller_id = ?",
                (machine, id_string, token.sender_machine, token.sender_id.as_str()),
            )
            .await
            .context("dedup lookup")?;
        match rows.next().await.context("dedup lookup row")? {
            Some(row) => Ok(row.get::<i64>(0).context("last_seq column")? >= token.seq),
            None => Ok(false),
        }
    }

    /// Un-failed outbox rows for one sender, in seq order (drain-on-activate).
    pub async fn pending_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
    ) -> anyhow::Result<Vec<OutboxRow>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT seq, target_machine, target_id_json, action, kind FROM outbox
                 WHERE sender_machine = ? AND sender_id = ? AND failure IS NULL
                 ORDER BY seq",
                (machine, sender_id),
            )
            .await
            .context("pending outbox")?;
        let mut pending = Vec::new();
        while let Some(row) = rows.next().await.context("pending outbox row")? {
            let kind: String = row.get(4).context("kind column")?;
            pending.push(OutboxRow {
                seq: row.get(0).context("seq column")?,
                kind: RowKind::parse(&kind)
                    .with_context(|| format!("unknown outbox row kind {kind}"))?,
                target_machine: row.get(1).context("target_machine column")?,
                target_id_json: row.get(2).context("target_id_json column")?,
                payload_json: row.get(3).context("action column")?,
            });
        }
        Ok(pending)
    }

    /// Sweep: entities whose sender outbox has un-failed rows older than the cutoff.
    /// Returns (machine, sender_id_json) pairs — the sweep wakes them; it never executes.
    /// Stable ordering + offset let the sweep page past a stuck head instead of starving
    /// everything behind it.
    pub async fn stalled_outbox_senders(
        &self,
        cutoff_ms: i64,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT sender_machine, sender_id_json FROM outbox
                 WHERE failure IS NULL AND created_at < ?
                 ORDER BY sender_machine, sender_id_json LIMIT ? OFFSET ?",
                (cutoff_ms, limit, offset),
            )
            .await
            .context("stalled outbox senders")?;
        let mut stalled = Vec::new();
        while let Some(row) = rows.next().await.context("stalled sender row")? {
            stalled.push((row.get(0).context("machine column")?, row.get(1).context("id_json column")?));
        }
        Ok(stalled)
    }

    /// Sweep: entities whose persisted timer deadline passed the cutoff.
    pub async fn due_timers(&self, cutoff_ms: i64, limit: i64, offset: i64) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT machine, id_json FROM entities
                 WHERE next_tick_on IS NOT NULL AND next_tick_on < ?
                 ORDER BY machine, id LIMIT ? OFFSET ?",
                (cutoff_ms, limit, offset),
            )
            .await
            .context("due timers")?;
        let mut due = Vec::new();
        while let Some(row) = rows.next().await.context("due timer row")? {
            due.push((row.get(0).context("machine column")?, row.get(1).context("id_json column")?));
        }
        Ok(due)
    }

    /// Ack: the receiver definitively applied (or deduped) this row — delete it.
    pub async fn ack_outbox(&self, machine: &'static str, sender_id: &str, seq: i64) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM outbox WHERE sender_machine = ? AND sender_id = ? AND seq = ?",
            (machine, sender_id, seq),
        )
        .await
        .context("ack outbox")?;
        Ok(())
    }

    /// Poison: the receiver rejected this row (domain error) — keep it, marked, out of pending reads.
    pub async fn fail_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
        seq: i64,
        reason: &str,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE outbox SET failure = ? WHERE sender_machine = ? AND sender_id = ? AND seq = ?",
            (reason, machine, sender_id, seq),
        )
        .await
        .context("fail outbox")?;
        Ok(())
    }

    pub async fn delete(&self, machine: &'static str, id_string: &str) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute("BEGIN IMMEDIATE", ()).await.context("begin delete")?;
        let result = async {
            conn.execute(
                "DELETE FROM entities WHERE machine = ? AND id = ?",
                (machine, id_string),
            )
            .await
            .context("delete entity")?;
            conn.execute(
                "DELETE FROM outbox WHERE sender_machine = ? AND sender_id = ?",
                (machine, id_string),
            )
            .await
            .context("delete outbox")?;
            conn.execute(
                "DELETE FROM call_dedup WHERE machine = ? AND id = ?",
                (machine, id_string),
            )
            .await
            .context("delete dedup")?;
            // also the trail it left as a CALLER in other entities' slots — a recreated
            // sender restarts at seq 0, and a stale last_seq would swallow its first sends
            conn.execute(
                "DELETE FROM call_dedup WHERE caller_machine = ? AND caller_id = ?",
                (machine, id_string),
            )
            .await
            .context("delete caller-side dedup")?;
            Ok(SaveOutcome::Ok)
        }
        .await;
        finish_tx(&conn, result).await.map(|_| ())
    }
}

async fn insert_in_tx(
    conn: &turso::Connection,
    machine: &'static str,
    id_string: &str,
    id_json: &str,
    state_json: &str,
    next_tick_on: Option<i64>,
    outbox: &[OutboxDraft],
) -> anyhow::Result<SaveOutcome> {
    let mut existing = conn
        .query(
            "SELECT version FROM entities WHERE machine = ? AND id = ?",
            (machine, id_string),
        )
        .await
        .context("insert pre-check")?;
    if let Some(row) = existing.next().await.context("insert pre-check row")? {
        let actual: i64 = row.get(0).context("version column")?;
        return Ok(SaveOutcome::Conflict { actual: Some(actual) });
    }
    conn.execute(
        "INSERT INTO entities (machine, id, id_json, state, version, next_outbox_seq, next_tick_on)
         VALUES (?, ?, ?, ?, 0, ?, ?)",
        (
            machine,
            id_string,
            id_json,
            state_json,
            outbox.len() as i64,
            turso::Value::from(next_tick_on),
        ),
    )
    .await
    .context("insert entity")?;
    insert_outbox_rows(conn, machine, id_string, id_json, 0, outbox).await?;
    Ok(SaveOutcome::Ok)
}

async fn save_in_tx(
    conn: &turso::Connection,
    write: &TransitionWrite,
    id_json: &str,
) -> anyhow::Result<SaveOutcome> {
    let updated = conn
        .execute(
            "UPDATE entities SET state = ?, version = version + 1, next_outbox_seq = ?, next_tick_on = ?
             WHERE machine = ? AND id = ? AND version = ?",
            (
                write.state_json.as_str(),
                write.next_outbox_seq,
                turso::Value::from(write.next_tick_on),
                write.machine,
                write.id_string.as_str(),
                write.expected_version,
            ),
        )
        .await
        .context("CAS update")?;
    if updated == 0 {
        let mut rows = conn
            .query(
                "SELECT version FROM entities WHERE machine = ? AND id = ?",
                (write.machine, write.id_string.as_str()),
            )
            .await
            .context("conflict probe")?;
        let actual = match rows.next().await.context("conflict probe row")? {
            Some(row) => Some(row.get::<i64>(0).context("version column")?),
            None => None,
        };
        return Ok(SaveOutcome::Conflict { actual });
    }
    insert_outbox_rows(conn, write.machine, &write.id_string, id_json, write.first_seq, &write.outbox)
        .await?;
    if let Some(token) = &write.dedup {
        let changed = conn
            .execute(
                "UPDATE call_dedup SET last_seq = ?
                 WHERE machine = ? AND id = ? AND caller_machine = ? AND caller_id = ?",
                (
                    token.seq,
                    write.machine,
                    write.id_string.as_str(),
                    token.sender_machine,
                    token.sender_id.as_str(),
                ),
            )
            .await
            .context("dedup update")?;
        if changed == 0 {
            conn.execute(
                "INSERT INTO call_dedup (machine, id, caller_machine, caller_id, last_seq)
                 VALUES (?, ?, ?, ?, ?)",
                (
                    write.machine,
                    write.id_string.as_str(),
                    token.sender_machine,
                    token.sender_id.as_str(),
                    token.seq,
                ),
            )
            .await
            .context("dedup insert")?;
        }
    }
    Ok(SaveOutcome::Ok)
}

async fn insert_outbox_rows(
    conn: &turso::Connection,
    machine: &'static str,
    id_string: &str,
    id_json: &str,
    first_seq: i64,
    outbox: &[OutboxDraft],
) -> anyhow::Result<()> {
    for (offset, draft) in outbox.iter().enumerate() {
        conn.execute(
            "INSERT INTO outbox (sender_machine, sender_id, seq, sender_id_json, target_machine, target_id_json, action, kind, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                machine,
                id_string,
                first_seq + offset as i64,
                id_json,
                draft.target_machine,
                draft.target_id_json.as_str(),
                draft.payload_json.as_str(),
                draft.kind.as_str(),
                chrono::Utc::now().timestamp_millis(),
            ),
        )
        .await
        .context("insert outbox row")?;
    }
    Ok(())
}

async fn finish_tx(
    conn: &turso::Connection,
    result: anyhow::Result<SaveOutcome>,
) -> anyhow::Result<SaveOutcome> {
    match &result {
        Ok(SaveOutcome::Ok) => {
            conn.execute("COMMIT", ()).await.context("commit")?;
        }
        Ok(SaveOutcome::Conflict { .. }) | Err(_) => {
            let _ = conn.execute("ROLLBACK", ()).await;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_store(tag: &str) -> Store {
        let path = std::env::temp_dir().join(format!("re_fw_store_test_{}_{tag}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = turso::Builder::new_local(path.to_str().expect("utf8 temp path"))
            .build()
            .await
            .expect("open test db");
        create_tables(&db).await.expect("create tables");
        Store { db }
    }

    fn write(expected_version: i64, first_seq: i64, outbox: Vec<OutboxDraft>, dedup: Option<CallToken>) -> TransitionWrite {
        TransitionWrite {
            machine: "TestMachine",
            id_string: "e1".to_string(),
            state_json: format!("\"v{}\"", expected_version + 1),
            expected_version,
            first_seq,
            next_outbox_seq: first_seq + outbox.len() as i64,
            next_tick_on: None,
            outbox,
            dedup,
        }
    }

    #[tokio::test]
    async fn cas_roundtrip_and_conflict() {
        let store = fresh_store("cas").await;

        assert!(store.load("TestMachine", "e1").await.expect("load").is_none());
        assert!(matches!(
            store.insert("TestMachine", "e1", "\"e1\"", "\"v0\"", None, &[]).await.expect("insert"),
            SaveOutcome::Ok
        ));
        assert!(matches!(
            store.insert("TestMachine", "e1", "\"e1\"", "\"v0\"", None, &[]).await.expect("re-insert"),
            SaveOutcome::Conflict { actual: Some(0) }
        ));

        let loaded = store.load("TestMachine", "e1").await.expect("load").expect("exists");
        assert_eq!((loaded.version, loaded.next_outbox_seq), (0, 0));

        assert!(matches!(
            store.save(&write(0, 0, Vec::new(), None), "\"e1\"").await.expect("save"),
            SaveOutcome::Ok
        ));
        // stale version → conflict, and the failed transaction must leave no trace
        assert!(matches!(
            store.save(&write(0, 0, Vec::new(), None), "\"e1\"").await.expect("stale save"),
            SaveOutcome::Conflict { actual: Some(1) }
        ));
        let loaded = store.load("TestMachine", "e1").await.expect("load").expect("exists");
        assert_eq!((loaded.version, loaded.state_json.as_str()), (1, "\"v1\""));

        store.delete("TestMachine", "e1").await.expect("delete");
        assert!(store.load("TestMachine", "e1").await.expect("load").is_none());
    }

    #[tokio::test]
    async fn outbox_lifecycle_and_dedup() {
        let store = fresh_store("outbox").await;
        store.insert("TestMachine", "e1", "\"e1\"", "\"v0\"", None, &[]).await.expect("insert");

        let drafts = vec![
            OutboxDraft {
                kind: RowKind::Act,
                target_machine: "Other",
                target_id_json: "\"t1\"".to_string(),
                payload_json: "\"a1\"".to_string(),
            },
            OutboxDraft {
                kind: RowKind::Construct,
                target_machine: "Other",
                target_id_json: "\"t1\"".to_string(),
                payload_json: "\"a2\"".to_string(),
            },
        ];
        let token = CallToken {
            sender_machine: "Caller",
            sender_id: "c9".to_string(),
            seq: 7,
        };
        assert!(matches!(
            store.save(&write(0, 0, drafts, Some(token.clone())), "\"e1\"").await.expect("save"),
            SaveOutcome::Ok
        ));

        // rows durable + ordered; senders visible for boot recovery; dedup row committed with them
        let pending = store.pending_outbox("TestMachine", "e1").await.expect("pending");
        assert_eq!(
            pending.iter().map(|r| (r.seq, r.kind, r.payload_json.as_str())).collect::<Vec<_>>(),
            vec![(0, RowKind::Act, "\"a1\""), (1, RowKind::Construct, "\"a2\"")]
        );
        let far_future = chrono::Utc::now().timestamp_millis() + 3_600_000;
        assert_eq!(
            store.stalled_outbox_senders(far_future, 10, 0).await.expect("stalled"),
            vec![("TestMachine".to_string(), "\"e1\"".to_string())]
        );
        assert!(store.is_duplicate("TestMachine", "e1", &token).await.expect("dup"));
        // monotonic: anything at or below the slot is a duplicate; above it is fresh
        assert!(store
            .is_duplicate("TestMachine", "e1", &CallToken { seq: 6, ..token.clone() })
            .await
            .expect("dup"));
        assert!(!store
            .is_duplicate("TestMachine", "e1", &CallToken { seq: 8, ..token.clone() })
            .await
            .expect("dup"));

        // ack deletes; poison hides from pending but keeps the row
        store.ack_outbox("TestMachine", "e1", 0).await.expect("ack");
        store.fail_outbox("TestMachine", "e1", 1, "boom").await.expect("fail");
        assert!(store.pending_outbox("TestMachine", "e1").await.expect("pending").is_empty());
        assert!(store.stalled_outbox_senders(far_future, 10, 0).await.expect("stalled").is_empty());

        // deleting the CALLER clears the dedup trail it left on other entities
        store.delete("Caller", "c9").await.expect("delete caller");
        assert!(!store.is_duplicate("TestMachine", "e1", &token).await.expect("dup"));
    }

    #[tokio::test]
    async fn timer_deadlines() {
        let store = fresh_store("timers").await;
        store
            .insert("TestMachine", "e1", "\"e1\"", "\"v0\"", Some(1_000), &[])
            .await
            .expect("insert");

        let due = |cutoff| store.due_timers(cutoff, 10, 0);
        assert_eq!(due(2_000).await.expect("due"), vec![("TestMachine".to_string(), "\"e1\"".to_string())]);
        assert!(due(500).await.expect("due").is_empty(), "not yet due");

        // a transition that schedules nothing clears the persisted deadline
        assert!(matches!(
            store.save(&write(0, 0, Vec::new(), None), "\"e1\"").await.expect("save"),
            SaveOutcome::Ok
        ));
        assert!(due(i64::MAX).await.expect("due").is_empty(), "deadline cleared on commit");
    }
}
