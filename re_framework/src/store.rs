//! Persistence backends behind one abstraction (#186 pass 1). The store — not the in-memory
//! registry — is the transaction guarantee: state CAS, outbox inserts, and the receiver dedup
//! upsert commit atomically. `Turso` is the real backend; `Json` preserves the legacy
//! file-per-entity behavior (no CAS, no durable outbox) for callers that never init a store.

use crate::machine::{EntityId, StateMachine};
use anyhow::Context;
use std::sync::OnceLock;

/// An internal entity→entity action captured during a transition, already serialized —
/// first-class data, unlike external effects which stay opaque futures.
#[derive(Debug, Clone)]
pub(crate) struct OutboxDraft {
    pub target_machine: &'static str,
    pub target_id_json: String,
    pub action_json: String,
}

/// A durable outbox row loaded back from the store (drain-on-activate / boot recovery).
#[derive(Debug, Clone)]
pub(crate) struct OutboxRow {
    pub seq: i64,
    pub target_machine: String,
    pub target_id_json: String,
    pub action_json: String,
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

pub(crate) enum Store {
    Json,
    Turso(turso::Database),
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
    let conn = db.connect().context("connect for schema init")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entities (
             machine TEXT NOT NULL,
             id TEXT NOT NULL,
             state TEXT NOT NULL,
             version INTEGER NOT NULL,
             next_outbox_seq INTEGER NOT NULL,
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
    .context("create framework tables")?;
    STORE
        .set(Store::Turso(db))
        .map_err(|_| anyhow::anyhow!("store already initialized (or already used as Json)"))
}

pub(crate) fn store() -> &'static Store {
    STORE.get_or_init(|| Store::Json)
}

impl Store {
    fn connect(db: &turso::Database) -> anyhow::Result<turso::Connection> {
        let conn = db.connect().context("turso connect")?;
        // concurrent actors write from separate connections; wait for the lock instead of failing busy
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("set busy_timeout")?;
        Ok(conn)
    }

    pub async fn load<SM: StateMachine>(&self, id: &SM::Id) -> anyhow::Result<Option<LoadedEntity>> {
        match self {
            Store::Json => Ok(json_load::<SM>(id).map(|state_json| LoadedEntity {
                state_json,
                version: 0,
                next_outbox_seq: 0,
            })),
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
                let mut rows = conn
                    .query(
                        "SELECT state, version, next_outbox_seq FROM entities WHERE machine = ? AND id = ?",
                        (SM::name(), id.get_id_string()),
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
        }
    }

    /// Construct-time insert. `Conflict` means the row already exists (lost a construct race).
    pub async fn insert(
        &self,
        machine: &'static str,
        id_string: &str,
        id_json: &str,
        state_json: &str,
        outbox: &[OutboxDraft],
    ) -> anyhow::Result<SaveOutcome> {
        match self {
            Store::Json => {
                json_persist(machine, id_string, state_json)?;
                Ok(SaveOutcome::Ok)
            }
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
                conn.execute("BEGIN IMMEDIATE", ()).await.context("begin insert")?;
                let result = self
                    .insert_in_tx(&conn, machine, id_string, id_json, state_json, outbox)
                    .await;
                finish_tx(&conn, result).await
            }
        }
    }

    async fn insert_in_tx(
        &self,
        conn: &turso::Connection,
        machine: &'static str,
        id_string: &str,
        id_json: &str,
        state_json: &str,
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
            "INSERT INTO entities (machine, id, state, version, next_outbox_seq) VALUES (?, ?, ?, 0, ?)",
            (machine, id_string, state_json, outbox.len() as i64),
        )
        .await
        .context("insert entity")?;
        insert_outbox_rows(conn, machine, id_string, id_json, 0, outbox).await?;
        Ok(SaveOutcome::Ok)
    }

    /// The atomic transition unit: {state CAS + outbox inserts + dedup upsert}, commit-or-nothing.
    pub async fn save(&self, write: &TransitionWrite, id_json: &str) -> anyhow::Result<SaveOutcome> {
        match self {
            Store::Json => {
                json_persist(write.machine, &write.id_string, &write.state_json)?;
                Ok(SaveOutcome::Ok)
            }
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
                conn.execute("BEGIN IMMEDIATE", ()).await.context("begin save")?;
                let result = self.save_in_tx(&conn, write, id_json).await;
                finish_tx(&conn, result).await
            }
        }
    }

    async fn save_in_tx(
        &self,
        conn: &turso::Connection,
        write: &TransitionWrite,
        id_json: &str,
    ) -> anyhow::Result<SaveOutcome> {
        let updated = conn
            .execute(
                "UPDATE entities SET state = ?, version = version + 1, next_outbox_seq = ?
                 WHERE machine = ? AND id = ? AND version = ?",
                (
                    write.state_json.as_str(),
                    write.next_outbox_seq,
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

    /// Has this exact call already been applied? (Receiver-side check before transitioning.)
    pub async fn is_duplicate(
        &self,
        machine: &'static str,
        id_string: &str,
        token: &CallToken,
    ) -> anyhow::Result<bool> {
        match self {
            Store::Json => Ok(false),
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
                let mut rows = conn
                    .query(
                        "SELECT last_seq FROM call_dedup
                         WHERE machine = ? AND id = ? AND caller_machine = ? AND caller_id = ?",
                        (machine, id_string, token.sender_machine, token.sender_id.as_str()),
                    )
                    .await
                    .context("dedup lookup")?;
                match rows.next().await.context("dedup lookup row")? {
                    Some(row) => Ok(row.get::<i64>(0).context("last_seq column")? == token.seq),
                    None => Ok(false),
                }
            }
        }
    }

    /// Un-failed outbox rows for one sender, in seq order (drain-on-activate).
    pub async fn pending_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
    ) -> anyhow::Result<Vec<OutboxRow>> {
        match self {
            Store::Json => Ok(Vec::new()),
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
                let mut rows = conn
                    .query(
                        "SELECT seq, target_machine, target_id_json, action FROM outbox
                         WHERE sender_machine = ? AND sender_id = ? AND failure IS NULL
                         ORDER BY seq",
                        (machine, sender_id),
                    )
                    .await
                    .context("pending outbox")?;
                let mut pending = Vec::new();
                while let Some(row) = rows.next().await.context("pending outbox row")? {
                    pending.push(OutboxRow {
                        seq: row.get(0).context("seq column")?,
                        target_machine: row.get(1).context("target_machine column")?,
                        target_id_json: row.get(2).context("target_id_json column")?,
                        action_json: row.get(3).context("action column")?,
                    });
                }
                Ok(pending)
            }
        }
    }

    /// Senders of this machine type with un-failed pending rows (boot recovery), as id JSON.
    pub async fn pending_senders(&self, machine: &'static str) -> anyhow::Result<Vec<String>> {
        match self {
            Store::Json => Ok(Vec::new()),
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
                let mut rows = conn
                    .query(
                        "SELECT DISTINCT sender_id_json FROM outbox
                         WHERE sender_machine = ? AND failure IS NULL",
                        (machine,),
                    )
                    .await
                    .context("pending senders")?;
                let mut senders = Vec::new();
                while let Some(row) = rows.next().await.context("pending senders row")? {
                    senders.push(row.get(0).context("sender_id_json column")?);
                }
                Ok(senders)
            }
        }
    }

    /// Ack: the receiver definitively applied (or deduped) this row — delete it.
    pub async fn ack_outbox(&self, machine: &'static str, sender_id: &str, seq: i64) -> anyhow::Result<()> {
        match self {
            Store::Json => Ok(()),
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
                conn.execute(
                    "DELETE FROM outbox WHERE sender_machine = ? AND sender_id = ? AND seq = ?",
                    (machine, sender_id, seq),
                )
                .await
                .context("ack outbox")?;
                Ok(())
            }
        }
    }

    /// Poison: the receiver rejected this row (domain error) — keep it, marked, out of pending reads.
    pub async fn fail_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
        seq: i64,
        reason: &str,
    ) -> anyhow::Result<()> {
        match self {
            Store::Json => Ok(()),
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
                conn.execute(
                    "UPDATE outbox SET failure = ? WHERE sender_machine = ? AND sender_id = ? AND seq = ?",
                    (reason, machine, sender_id, seq),
                )
                .await
                .context("fail outbox")?;
                Ok(())
            }
        }
    }

    pub async fn delete(&self, machine: &'static str, id_string: &str) -> anyhow::Result<()> {
        match self {
            Store::Json => json_delete(machine, id_string),
            Store::Turso(db) => {
                let conn = Self::connect(db)?;
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
                    Ok(SaveOutcome::Ok)
                }
                .await;
                finish_tx(&conn, result).await.map(|_| ())
            }
        }
    }
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
            "INSERT INTO outbox (sender_machine, sender_id, seq, sender_id_json, target_machine, target_id_json, action)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                machine,
                id_string,
                first_seq + offset as i64,
                id_json,
                draft.target_machine,
                draft.target_id_json.as_str(),
                draft.action_json.as_str(),
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

// ---- legacy JSON backend (behavior-preserving fallback; no CAS, no durable outbox) ----

fn json_path(machine: &str, id_string: &str) -> std::path::PathBuf {
    let safe_id: String = id_string
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    std::path::Path::new("framework_db")
        .join(machine)
        .join(format!("{safe_id}.json"))
}

fn json_persist(machine: &str, id_string: &str, state_json: &str) -> anyhow::Result<()> {
    let path = json_path(machine, id_string);
    let dir = path.parent().expect("entity path always has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("create_dir_all {}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, state_json.as_bytes()).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

fn json_load<SM: StateMachine>(id: &SM::Id) -> Option<String> {
    std::fs::read_to_string(json_path(SM::name(), &id.get_id_string())).ok()
}

fn json_delete(machine: &str, id_string: &str) -> anyhow::Result<()> {
    let path = json_path(machine, id_string);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{EntityId, Identified, Scheduled, StateMachine};
    use crate::{Effects, StateMachineHandle};
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    struct TestMachine;
    #[derive(Clone, Serialize, Deserialize)]
    struct TestState;
    #[derive(Debug, Serialize, Deserialize)]
    enum TestAction {
        Noop,
    }
    struct TestInit {
        id: String,
    }
    impl Identified for TestInit {
        type Id = String;
        fn get_id(&self) -> &String {
            &self.id
        }
    }
    impl StateMachine for TestMachine {
        type State = TestState;
        type Id = String;
        type Action = TestAction;
        type Construction = TestInit;
        type Env = ();
        fn construct(_: TestInit, _: &mut Effects<Self>) -> TestState {
            TestState
        }
        fn transition(
            state: &TestState,
            _: &String,
            _: &Arc<()>,
            _: &TestAction,
            _: &mut Effects<Self>,
        ) -> anyhow::Result<TestState> {
            Ok(state.clone())
        }
        fn schedule(_: &TestState) -> Option<Scheduled<TestAction>> {
            None
        }
        fn handle() -> &'static StateMachineHandle<TestMachine> {
            unreachable!("store tests never dispatch")
        }
        fn name() -> &'static str {
            "TestMachine"
        }
    }

    async fn fresh_store(tag: &str) -> Store {
        let path = std::env::temp_dir().join(format!("re_fw_store_test_{}_{tag}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = turso::Builder::new_local(path.to_str().expect("utf8 temp path"))
            .build()
            .await
            .expect("open test db");
        let conn = db.connect().expect("connect");
        conn.execute_batch(
            "CREATE TABLE entities (machine TEXT NOT NULL, id TEXT NOT NULL, state TEXT NOT NULL, version INTEGER NOT NULL, next_outbox_seq INTEGER NOT NULL, PRIMARY KEY (machine, id));
             CREATE TABLE outbox (sender_machine TEXT NOT NULL, sender_id TEXT NOT NULL, seq INTEGER NOT NULL, sender_id_json TEXT NOT NULL, target_machine TEXT NOT NULL, target_id_json TEXT NOT NULL, action TEXT NOT NULL, failure TEXT, PRIMARY KEY (sender_machine, sender_id, seq));
             CREATE TABLE call_dedup (machine TEXT NOT NULL, id TEXT NOT NULL, caller_machine TEXT NOT NULL, caller_id TEXT NOT NULL, last_seq INTEGER NOT NULL, PRIMARY KEY (machine, id, caller_machine, caller_id));",
        )
        .await
        .expect("create tables");
        Store::Turso(db)
    }

    fn write(expected_version: i64, first_seq: i64, outbox: Vec<OutboxDraft>, dedup: Option<CallToken>) -> TransitionWrite {
        TransitionWrite {
            machine: "TestMachine",
            id_string: "e1".to_string(),
            state_json: format!("\"v{}\"", expected_version + 1),
            expected_version,
            first_seq,
            next_outbox_seq: first_seq + outbox.len() as i64,
            outbox,
            dedup,
        }
    }

    #[tokio::test]
    async fn cas_roundtrip_and_conflict() {
        let store = fresh_store("cas").await;
        let id = "e1".to_string();

        assert!(store.load::<TestMachine>(&id).await.expect("load").is_none());
        assert!(matches!(
            store.insert("TestMachine", "e1", "\"e1\"", "\"v0\"", &[]).await.expect("insert"),
            SaveOutcome::Ok
        ));
        assert!(matches!(
            store.insert("TestMachine", "e1", "\"e1\"", "\"v0\"", &[]).await.expect("re-insert"),
            SaveOutcome::Conflict { actual: Some(0) }
        ));

        let loaded = store.load::<TestMachine>(&id).await.expect("load").expect("exists");
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
        let loaded = store.load::<TestMachine>(&id).await.expect("load").expect("exists");
        assert_eq!((loaded.version, loaded.state_json.as_str()), (1, "\"v1\""));

        store.delete("TestMachine", "e1").await.expect("delete");
        assert!(store.load::<TestMachine>(&id).await.expect("load").is_none());
    }

    #[tokio::test]
    async fn outbox_lifecycle_and_dedup() {
        let store = fresh_store("outbox").await;
        store.insert("TestMachine", "e1", "\"e1\"", "\"v0\"", &[]).await.expect("insert");

        let drafts = vec![
            OutboxDraft {
                target_machine: "Other",
                target_id_json: "\"t1\"".to_string(),
                action_json: "\"a1\"".to_string(),
            },
            OutboxDraft {
                target_machine: "Other",
                target_id_json: "\"t1\"".to_string(),
                action_json: "\"a2\"".to_string(),
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
            pending.iter().map(|r| (r.seq, r.action_json.as_str())).collect::<Vec<_>>(),
            vec![(0, "\"a1\""), (1, "\"a2\"")]
        );
        assert_eq!(store.pending_senders("TestMachine").await.expect("senders"), vec!["\"e1\"".to_string()]);
        assert!(store.is_duplicate("TestMachine", "e1", &token).await.expect("dup"));
        assert!(!store
            .is_duplicate("TestMachine", "e1", &CallToken { seq: 8, ..token.clone() })
            .await
            .expect("dup"));

        // ack deletes; poison hides from pending but keeps the row
        store.ack_outbox("TestMachine", "e1", 0).await.expect("ack");
        store.fail_outbox("TestMachine", "e1", 1, "boom").await.expect("fail");
        assert!(store.pending_outbox("TestMachine", "e1").await.expect("pending").is_empty());
        assert!(store.pending_senders("TestMachine").await.expect("senders").is_empty());
    }
}
