
use async_trait::async_trait;
use std::sync::OnceLock;

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

#[derive(Debug, Clone)]
pub(crate) struct OutboxDraft {
    pub kind: RowKind,
    pub target_machine: &'static str,
    pub target_id_json: String,
    pub payload_json: String,
}

pub(crate) fn new_generation() -> i64 {
    static NEXT: OnceLock<std::sync::atomic::AtomicI64> = OnceLock::new();
    NEXT.get_or_init(|| std::sync::atomic::AtomicI64::new(chrono::Utc::now().timestamp_micros()))
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub(crate) struct OutboxRow {
    pub seq: i64,
    pub sender_generation: i64,
    pub kind: RowKind,
    pub target_machine: String,
    pub target_id_json: String,
    pub payload_json: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CallToken {
    pub sender_machine: &'static str,
    pub sender_id: String,
    pub sender_generation: i64,
    pub seq: i64,
}

pub(crate) struct TransitionWrite {
    pub machine: &'static str,
    pub id_string: String,
    pub id_json: String,
    pub state_json: String,
    pub generation: i64,
    pub expected_version: i64,
    pub first_seq: i64,
    pub next_outbox_seq: i64,
    pub next_tick_on: Option<i64>,
    pub outbox: Vec<OutboxDraft>,
    pub dedup: Option<CallToken>,
}

pub(crate) enum SaveOutcome {
    Ok,
    Conflict { actual: Option<i64> },
}

pub(crate) struct LoadedEntity {
    pub state_json: String,
    pub generation: i64,
    pub version: i64,
    pub next_outbox_seq: i64,
}

#[async_trait]
pub(crate) trait Store: Send + Sync {
    async fn load(&self, machine: &'static str, id_string: &str) -> anyhow::Result<Option<LoadedEntity>>;

    #[allow(clippy::too_many_arguments)]
    async fn insert(
        &self,
        machine: &'static str,
        id_string: &str,
        id_json: &str,
        generation: i64,
        state_json: &str,
        next_tick_on: Option<i64>,
        outbox: &[OutboxDraft],
    ) -> anyhow::Result<SaveOutcome>;

    async fn save(&self, write: &TransitionWrite) -> anyhow::Result<SaveOutcome>;

    async fn is_duplicate(
        &self,
        machine: &'static str,
        id_string: &str,
        token: &CallToken,
    ) -> anyhow::Result<bool>;

    async fn pending_outbox(&self, machine: &'static str, sender_id: &str) -> anyhow::Result<Vec<OutboxRow>>;

    async fn stalled_outbox_senders(
        &self,
        cutoff_ms: i64,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<(String, String)>>;

    async fn due_timers(&self, cutoff_ms: i64, limit: i64, offset: i64) -> anyhow::Result<Vec<(String, String)>>;

    async fn ack_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
        sender_generation: i64,
        seq: i64,
    ) -> anyhow::Result<()>;

    async fn fail_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
        sender_generation: i64,
        seq: i64,
        reason: &str,
    ) -> anyhow::Result<()>;

    async fn delete(&self, machine: &'static str, id_string: &str) -> anyhow::Result<()>;
}

static STORE: OnceLock<Box<dyn Store>> = OnceLock::new();

pub(crate) fn init_store(backend: impl Store + 'static) -> anyhow::Result<()> {
    let boxed: Box<dyn Store> = Box::new(backend);
    STORE
        .set(boxed)
        .map_err(|_| anyhow::anyhow!("store already initialized"))
}

pub(crate) fn store() -> &'static dyn Store {
    STORE
        .get()
        .expect("store not initialized — call re_framework::init_turso_store before using state machines")
        .as_ref()
}

#[cfg(test)]
pub(crate) mod contract {
    use super::*;

    pub const GEN: i64 = 1;

    pub fn write(expected_version: i64, first_seq: i64, outbox: Vec<OutboxDraft>, dedup: Option<CallToken>) -> TransitionWrite {
        TransitionWrite {
            machine: "TestMachine",
            id_string: "e1".to_string(),
            id_json: "\"e1\"".to_string(),
            state_json: format!("\"v{}\"", expected_version + 1),
            generation: GEN,
            expected_version,
            first_seq,
            next_outbox_seq: first_seq + outbox.len() as i64,
            next_tick_on: None,
            outbox,
            dedup,
        }
    }

    pub async fn cas_roundtrip_and_conflict(store: &dyn Store) {
        assert!(store.load("TestMachine", "e1").await.expect("load").is_none());
        assert!(matches!(
            store.insert("TestMachine", "e1", "\"e1\"", GEN, "\"v0\"", None, &[]).await.expect("insert"),
            SaveOutcome::Ok
        ));
        assert!(matches!(
            store.insert("TestMachine", "e1", "\"e1\"", GEN, "\"v0\"", None, &[]).await.expect("re-insert"),
            SaveOutcome::Conflict { actual: Some(0) }
        ));

        let loaded = store.load("TestMachine", "e1").await.expect("load").expect("exists");
        assert_eq!((loaded.version, loaded.next_outbox_seq), (0, 0));

        assert!(matches!(
            store.save(&write(0, 0, Vec::new(), None)).await.expect("save"),
            SaveOutcome::Ok
        ));
        assert!(matches!(
            store.save(&write(0, 0, Vec::new(), None)).await.expect("stale save"),
            SaveOutcome::Conflict { actual: Some(1) }
        ));
        let loaded = store.load("TestMachine", "e1").await.expect("load").expect("exists");
        assert_eq!((loaded.version, loaded.state_json.as_str()), (1, "\"v1\""));

        store.delete("TestMachine", "e1").await.expect("delete");
        assert!(store.load("TestMachine", "e1").await.expect("load").is_none());
    }

    pub async fn outbox_lifecycle_and_dedup(store: &dyn Store) {
        store.insert("TestMachine", "e1", "\"e1\"", GEN, "\"v0\"", None, &[]).await.expect("insert");

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
            sender_generation: 5,
            seq: 7,
        };
        assert!(matches!(
            store.save(&write(0, 0, drafts, Some(token.clone()))).await.expect("save"),
            SaveOutcome::Ok
        ));

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
        assert!(store
            .is_duplicate("TestMachine", "e1", &CallToken { seq: 6, ..token.clone() })
            .await
            .expect("dup"));
        assert!(!store
            .is_duplicate("TestMachine", "e1", &CallToken { seq: 8, ..token.clone() })
            .await
            .expect("dup"));
        assert!(store
            .is_duplicate("TestMachine", "e1", &CallToken { sender_generation: 4, seq: 99, ..token.clone() })
            .await
            .expect("dup"));
        assert!(!store
            .is_duplicate("TestMachine", "e1", &CallToken { sender_generation: 6, seq: 0, ..token.clone() })
            .await
            .expect("dup"));

        let regress = CallToken { seq: 6, ..token.clone() };
        assert!(matches!(
            store.save(&write(1, 2, Vec::new(), Some(regress))).await.expect("save"),
            SaveOutcome::Ok
        ));
        assert!(store.is_duplicate("TestMachine", "e1", &token).await.expect("dup"));
        assert!(!store
            .is_duplicate("TestMachine", "e1", &CallToken { seq: 8, ..token.clone() })
            .await
            .expect("dup"));

        let next_gen = CallToken { sender_generation: 6, seq: 0, ..token.clone() };
        assert!(matches!(
            store.save(&write(2, 2, Vec::new(), Some(next_gen.clone()))).await.expect("save"),
            SaveOutcome::Ok
        ));
        assert!(store.is_duplicate("TestMachine", "e1", &next_gen).await.expect("dup"));
        assert!(!store
            .is_duplicate("TestMachine", "e1", &CallToken { sender_generation: 6, seq: 1, ..token.clone() })
            .await
            .expect("dup"));
        assert!(store.is_duplicate("TestMachine", "e1", &token).await.expect("dup"));

        store.ack_outbox("TestMachine", "e1", GEN + 1, 0).await.expect("stale ack");
        store.fail_outbox("TestMachine", "e1", GEN + 1, 1, "boom").await.expect("stale fail");
        assert_eq!(store.pending_outbox("TestMachine", "e1").await.expect("pending").len(), 2);

        store.ack_outbox("TestMachine", "e1", GEN, 0).await.expect("ack");
        store.fail_outbox("TestMachine", "e1", GEN, 1, "boom").await.expect("fail");
        assert!(store.pending_outbox("TestMachine", "e1").await.expect("pending").is_empty());
        assert!(store.stalled_outbox_senders(far_future, 10, 0).await.expect("stalled").is_empty());

        store.delete("Caller", "c9").await.expect("delete caller");
        assert!(!store.is_duplicate("TestMachine", "e1", &token).await.expect("dup"));
    }

    pub async fn generation_guards(store: &dyn Store) {
        store.insert("TestMachine", "e1", "\"e1\"", GEN, "\"v0\"", None, &[]).await.expect("insert");

        let zombie = TransitionWrite { generation: GEN + 1, ..write(0, 0, Vec::new(), None) };
        assert!(matches!(
            store.save(&zombie).await.expect("zombie save"),
            SaveOutcome::Conflict { actual: Some(0) }
        ));
        assert!(matches!(
            store.save(&write(0, 0, Vec::new(), None)).await.expect("save"),
            SaveOutcome::Ok
        ));
        let loaded = store.load("TestMachine", "e1").await.expect("load").expect("exists");
        assert_eq!((loaded.generation, loaded.version), (GEN, 1));
    }

    pub async fn timer_deadlines(store: &dyn Store) {
        store
            .insert("TestMachine", "e1", "\"e1\"", GEN, "\"v0\"", Some(1_000), &[])
            .await
            .expect("insert");

        let due = |cutoff| store.due_timers(cutoff, 10, 0);
        assert_eq!(due(2_000).await.expect("due"), vec![("TestMachine".to_string(), "\"e1\"".to_string())]);
        assert!(due(500).await.expect("due").is_empty(), "not yet due");

        assert!(matches!(
            store.save(&write(0, 0, Vec::new(), None)).await.expect("save"),
            SaveOutcome::Ok
        ));
        assert!(due(i64::MAX).await.expect("due").is_empty(), "deadline cleared on commit");
    }
}
