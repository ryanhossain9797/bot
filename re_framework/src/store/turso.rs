use crate::{
    error::ReFrameworkError,
    store::{
        CallToken, LoadedEntity, OutboxDraft, OutboxRow, RowKind, SaveOutcome, Store,
        TransitionWrite, init_store,
    },
};

use async_trait::async_trait;
use jiff::Timestamp;

pub(crate) struct TursoStore {
    db: turso::Database,
}

#[derive(Debug)]
pub enum StoreInitError {
    CreateDatabaseDirectoryFailed {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    OpenDatabaseFileFailed {
        path: String,
        source: turso::Error,
    },
    CreateFrameworkTablesFailed(turso::Error),
    StoreAlreadyInitialized,
}

impl std::fmt::Display for StoreInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreInitError::CreateDatabaseDirectoryFailed { path, .. } => {
                write!(f, "could not create database directory {}", path.display())
            }
            StoreInitError::OpenDatabaseFileFailed { path, .. } => {
                write!(f, "could not open turso database at {path}")
            }
            StoreInitError::CreateFrameworkTablesFailed(_) => {
                f.write_str("could not create framework tables")
            }
            StoreInitError::StoreAlreadyInitialized => f.write_str("store already initialized"),
        }
    }
}

impl std::error::Error for StoreInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreInitError::CreateDatabaseDirectoryFailed { source, .. } => Some(source),
            StoreInitError::OpenDatabaseFileFailed { source, .. } => Some(source),
            StoreInitError::CreateFrameworkTablesFailed(source) => Some(source),
            StoreInitError::StoreAlreadyInitialized => None,
        }
    }
}

pub async fn init_turso_store(path: &str) -> Result<(), StoreInitError> {
    let backend = open_turso_store(path).await?;
    init_store(backend).map_err(|_| StoreInitError::StoreAlreadyInitialized)
}

async fn open_turso_store(path: &str) -> Result<TursoStore, StoreInitError> {
    if let Some(dir) = std::path::Path::new(path)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir).map_err(|source| {
            StoreInitError::CreateDatabaseDirectoryFailed {
                path: dir.to_path_buf(),
                source,
            }
        })?;
    }
    let db = turso::Builder::new_local(path)
        .build()
        .await
        .map_err(|source| StoreInitError::OpenDatabaseFileFailed {
            path: path.to_string(),
            source,
        })?;
    create_tables(&db).await?;
    Ok(TursoStore { db })
}

async fn create_tables(db: &turso::Database) -> Result<(), StoreInitError> {
    let conn = db
        .connect()
        .map_err(StoreInitError::CreateFrameworkTablesFailed)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entities (
             machine TEXT NOT NULL,
             id TEXT NOT NULL,
             id_json TEXT NOT NULL,
             generation INTEGER NOT NULL,
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
             sender_generation INTEGER NOT NULL,
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
             caller_generation INTEGER NOT NULL,
             last_seq INTEGER NOT NULL,
             PRIMARY KEY (machine, id, caller_machine, caller_id)
         );",
    )
    .await
    .map_err(StoreInitError::CreateFrameworkTablesFailed)
}

impl TursoStore {
    fn connect(&self) -> Result<turso::Connection, ReFrameworkError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| ReFrameworkError::StoreError(format!("turso connect: {e}").into()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| {
                ReFrameworkError::StoreError(format!("turso set busy_timeout: {e}").into())
            })?;
        Ok(conn)
    }
}

#[async_trait]
impl Store for TursoStore {
    async fn load(
        &self,
        machine: &'static str,
        id_string: &str,
    ) -> Result<Option<LoadedEntity>, ReFrameworkError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT state, generation, version, next_outbox_seq FROM entities WHERE machine = ? AND id = ?",
                (machine, id_string),
            )
            .await
            .map_err(|e| ReFrameworkError::StoreError(format!("turso load entity: {e}").into()))?;
        match rows.next().await.map_err(|e| {
            ReFrameworkError::StoreError(format!("turso load entity row: {e}").into())
        })? {
            None => Ok(None),
            Some(row) => Ok(Some(LoadedEntity {
                state_json: row.get(0).map_err(|e| {
                    ReFrameworkError::StoreError(format!("turso state column: {e}").into())
                })?,
                generation: row.get(1).map_err(|e| {
                    ReFrameworkError::StoreError(format!("turso generation column: {e}").into())
                })?,
                version: row.get(2).map_err(|e| {
                    ReFrameworkError::StoreError(format!("turso version column: {e}").into())
                })?,
                next_outbox_seq: row.get(3).map_err(|e| {
                    ReFrameworkError::StoreError(
                        format!("turso next_outbox_seq column: {e}").into(),
                    )
                })?,
            })),
        }
    }

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
    ) -> Result<SaveOutcome, ReFrameworkError> {
        let conn = self.connect()?;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|e| ReFrameworkError::StoreError(format!("turso begin insert: {e}").into()))?;
        let result = insert_in_tx(
            &conn,
            machine,
            id_string,
            id_json,
            generation,
            state_json,
            next_tick_on,
            outbox,
        )
        .await;
        finish_tx(&conn, result).await
    }

    async fn save(&self, write: &TransitionWrite) -> Result<SaveOutcome, ReFrameworkError> {
        let conn = self.connect()?;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|e| ReFrameworkError::StoreError(format!("turso begin save: {e}").into()))?;
        let result = save_in_tx(&conn, write).await;
        finish_tx(&conn, result).await
    }

    async fn is_duplicate(
        &self,
        machine: &'static str,
        id_string: &str,
        token: &CallToken,
    ) -> Result<bool, ReFrameworkError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT caller_generation, last_seq FROM call_dedup
                 WHERE machine = ? AND id = ? AND caller_machine = ? AND caller_id = ?",
                (
                    machine,
                    id_string,
                    token.sender_machine,
                    token.sender_id.as_str(),
                ),
            )
            .await
            .map_err(|e| ReFrameworkError::StoreError(format!("turso dedup lookup: {e}").into()))?;
        match rows.next().await.map_err(|e| {
            ReFrameworkError::StoreError(format!("turso dedup lookup row: {e}").into())
        })? {
            None => Ok(false),
            Some(row) => {
                let slot_generation: i64 = row.get(0).map_err(|e| {
                    ReFrameworkError::StoreError(
                        format!("turso caller_generation column: {e}").into(),
                    )
                })?;
                let last_seq: i64 = row.get(1).map_err(|e| {
                    ReFrameworkError::StoreError(format!("turso last_seq column: {e}").into())
                })?;
                Ok(match slot_generation.cmp(&token.sender_generation) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Equal => last_seq >= token.seq,
                    std::cmp::Ordering::Less => false,
                })
            }
        }
    }

    async fn pending_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
    ) -> Result<Vec<OutboxRow>, ReFrameworkError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT seq, target_machine, target_id_json, action, kind, sender_generation FROM outbox
                 WHERE sender_machine = ? AND sender_id = ? AND failure IS NULL
                 ORDER BY seq",
                (machine, sender_id),
            )
            .await
            .map_err(|e| ReFrameworkError::StoreError(format!("turso pending outbox: {e}").into()))?;
        let mut pending = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| {
            ReFrameworkError::StoreError(format!("turso pending outbox row: {e}").into())
        })? {
            let kind: String = row.get(4).map_err(|e| {
                ReFrameworkError::StoreError(format!("turso kind column: {e}").into())
            })?;
            pending.push(OutboxRow {
                seq: row.get(0).map_err(|e| {
                    ReFrameworkError::StoreError(format!("turso seq column: {e}").into())
                })?,
                sender_generation: row.get(5).map_err(|e| {
                    ReFrameworkError::StoreError(
                        format!("turso sender_generation column: {e}").into(),
                    )
                })?,
                kind: RowKind::parse(&kind).ok_or_else(|| {
                    ReFrameworkError::StoreError(
                        format!("turso unknown outbox row kind {kind}").into(),
                    )
                })?,
                target_machine: row.get(1).map_err(|e| {
                    ReFrameworkError::StoreError(format!("turso target_machine column: {e}").into())
                })?,
                target_id_json: row.get(2).map_err(|e| {
                    ReFrameworkError::StoreError(format!("turso target_id_json column: {e}").into())
                })?,
                payload_json: row.get(3).map_err(|e| {
                    ReFrameworkError::StoreError(format!("turso action column: {e}").into())
                })?,
            });
        }
        Ok(pending)
    }

    async fn stalled_outbox_senders(
        &self,
        cutoff_ms: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<(String, String)>, ReFrameworkError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT sender_machine, sender_id_json FROM outbox
                 WHERE failure IS NULL AND created_at < ?
                 ORDER BY sender_machine, sender_id_json LIMIT ? OFFSET ?",
                (cutoff_ms, limit, offset),
            )
            .await
            .map_err(|e| {
                ReFrameworkError::StoreError(format!("turso stalled outbox senders: {e}").into())
            })?;
        collect_pairs(&mut rows).await
    }

    async fn due_timers(
        &self,
        cutoff_ms: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<(String, String)>, ReFrameworkError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT machine, id_json FROM entities
                 WHERE next_tick_on IS NOT NULL AND next_tick_on < ?
                 ORDER BY machine, id LIMIT ? OFFSET ?",
                (cutoff_ms, limit, offset),
            )
            .await
            .map_err(|e| ReFrameworkError::StoreError(format!("turso due timers: {e}").into()))?;
        collect_pairs(&mut rows).await
    }

    async fn ack_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
        sender_generation: i64,
        seq: i64,
    ) -> Result<(), ReFrameworkError> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM outbox
             WHERE sender_machine = ? AND sender_id = ? AND sender_generation = ? AND seq = ?",
            (machine, sender_id, sender_generation, seq),
        )
        .await
        .map_err(|e| ReFrameworkError::StoreError(format!("turso ack outbox: {e}").into()))?;
        Ok(())
    }

    async fn fail_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
        sender_generation: i64,
        seq: i64,
        reason: &str,
    ) -> Result<(), ReFrameworkError> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE outbox SET failure = ?
             WHERE sender_machine = ? AND sender_id = ? AND sender_generation = ? AND seq = ?",
            (reason, machine, sender_id, sender_generation, seq),
        )
        .await
        .map_err(|e| ReFrameworkError::StoreError(format!("turso fail outbox: {e}").into()))?;
        Ok(())
    }

    async fn delete(&self, machine: &'static str, id_string: &str) -> Result<(), ReFrameworkError> {
        let conn = self.connect()?;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|e| ReFrameworkError::StoreError(format!("turso begin delete: {e}").into()))?;
        let result = async {
            conn.execute(
                "DELETE FROM entities WHERE machine = ? AND id = ?",
                (machine, id_string),
            )
            .await
            .map_err(|e| {
                ReFrameworkError::StoreError(format!("turso delete entity: {e}").into())
            })?;
            conn.execute(
                "DELETE FROM outbox WHERE sender_machine = ? AND sender_id = ?",
                (machine, id_string),
            )
            .await
            .map_err(|e| {
                ReFrameworkError::StoreError(format!("turso delete outbox: {e}").into())
            })?;
            conn.execute(
                "DELETE FROM call_dedup WHERE machine = ? AND id = ?",
                (machine, id_string),
            )
            .await
            .map_err(|e| ReFrameworkError::StoreError(format!("turso delete dedup: {e}").into()))?;
            conn.execute(
                "DELETE FROM call_dedup WHERE caller_machine = ? AND caller_id = ?",
                (machine, id_string),
            )
            .await
            .map_err(|e| {
                ReFrameworkError::StoreError(format!("turso delete caller-side dedup: {e}").into())
            })?;
            Ok(SaveOutcome::Ok)
        }
        .await;
        finish_tx(&conn, result).await.map(|_| ())
    }
}

async fn collect_pairs(rows: &mut turso::Rows) -> Result<Vec<(String, String)>, ReFrameworkError> {
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| ReFrameworkError::StoreError(format!("turso pair row: {e}").into()))?
    {
        out.push((
            row.get(0)
                .map_err(|e| ReFrameworkError::StoreError(format!("turso column 0: {e}").into()))?,
            row.get(1)
                .map_err(|e| ReFrameworkError::StoreError(format!("turso column 1: {e}").into()))?,
        ));
    }
    Ok(out)
}

async fn current_version(
    conn: &turso::Connection,
    machine: &str,
    id: &str,
) -> Result<Option<i64>, ReFrameworkError> {
    let mut rows = conn
        .query(
            "SELECT version FROM entities WHERE machine = ? AND id = ?",
            (machine, id),
        )
        .await
        .map_err(|e| ReFrameworkError::StoreError(format!("turso version probe: {e}").into()))?;
    match rows
        .next()
        .await
        .map_err(|e| ReFrameworkError::StoreError(format!("turso version probe row: {e}").into()))?
    {
        Some(row) => Ok(Some(row.get(0).map_err(|e| {
            ReFrameworkError::StoreError(format!("turso version column: {e}").into())
        })?)),
        None => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_in_tx(
    conn: &turso::Connection,
    machine: &'static str,
    id_string: &str,
    id_json: &str,
    generation: i64,
    state_json: &str,
    next_tick_on: Option<i64>,
    outbox: &[OutboxDraft],
) -> Result<SaveOutcome, ReFrameworkError> {
    if let Some(actual) = current_version(conn, machine, id_string).await? {
        return Ok(SaveOutcome::Conflict {
            actual: Some(actual),
        });
    }
    conn.execute(
        "INSERT INTO entities (machine, id, id_json, generation, state, version, next_outbox_seq, next_tick_on)
         VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
        (
            machine,
            id_string,
            id_json,
            generation,
            state_json,
            outbox.len() as i64,
            turso::Value::from(next_tick_on),
        ),
    )
    .await
    .map_err(|e| ReFrameworkError::StoreError(format!("turso insert entity: {e}").into()))?;
    insert_outbox_rows(conn, machine, id_string, id_json, generation, 0, outbox).await?;
    Ok(SaveOutcome::Ok)
}

async fn save_in_tx(
    conn: &turso::Connection,
    write: &TransitionWrite,
) -> Result<SaveOutcome, ReFrameworkError> {
    let updated = conn
        .execute(
            "UPDATE entities SET state = ?, version = version + 1, next_outbox_seq = ?, next_tick_on = ?
             WHERE machine = ? AND id = ? AND version = ? AND generation = ?",
            (
                write.state_json.as_str(),
                write.next_outbox_seq,
                turso::Value::from(write.next_tick_on),
                write.machine,
                write.id_string.as_str(),
                write.expected_version,
                write.generation,
            ),
        )
        .await
        .map_err(|e| ReFrameworkError::StoreError(format!("turso CAS update: {e}").into()))?;
    if updated == 0 {
        let actual = current_version(conn, write.machine, &write.id_string).await?;
        return Ok(SaveOutcome::Conflict { actual });
    }
    insert_outbox_rows(
        conn,
        write.machine,
        &write.id_string,
        &write.id_json,
        write.generation,
        write.first_seq,
        &write.outbox,
    )
    .await?;
    if let Some(token) = &write.dedup {
        conn.execute(
            "INSERT INTO call_dedup (machine, id, caller_machine, caller_id, caller_generation, last_seq)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(machine, id, caller_machine, caller_id) DO UPDATE SET
                 last_seq = CASE WHEN caller_generation = excluded.caller_generation
                                 THEN MAX(last_seq, excluded.last_seq) ELSE excluded.last_seq END,
                 caller_generation = excluded.caller_generation",
            (
                write.machine,
                write.id_string.as_str(),
                token.sender_machine,
                token.sender_id.as_str(),
                token.sender_generation,
                token.seq,
            ),
        )
        .await
        .map_err(|e| ReFrameworkError::StoreError(format!("turso dedup upsert: {e}").into()))?;
    }
    Ok(SaveOutcome::Ok)
}

#[allow(clippy::too_many_arguments)]
async fn insert_outbox_rows(
    conn: &turso::Connection,
    machine: &'static str,
    id_string: &str,
    id_json: &str,
    generation: i64,
    first_seq: i64,
    outbox: &[OutboxDraft],
) -> Result<(), ReFrameworkError> {
    for (offset, draft) in outbox.iter().enumerate() {
        conn.execute(
            "INSERT INTO outbox (sender_machine, sender_id, seq, sender_generation, sender_id_json, target_machine, target_id_json, action, kind, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                machine,
                id_string,
                first_seq + offset as i64,
                generation,
                id_json,
                draft.target_machine,
                draft.target_id_json.as_str(),
                draft.payload_json.as_str(),
                draft.kind.as_str(),
                Timestamp::now().as_millisecond(),
            ),
        )
        .await
        .map_err(|e| ReFrameworkError::StoreError(format!("turso insert outbox row: {e}").into()))?;
    }
    Ok(())
}

async fn finish_tx(
    conn: &turso::Connection,
    result: Result<SaveOutcome, ReFrameworkError>,
) -> Result<SaveOutcome, ReFrameworkError> {
    match &result {
        Ok(SaveOutcome::Ok) => {
            conn.execute("COMMIT", ())
                .await
                .map_err(|e| ReFrameworkError::StoreError(format!("turso commit: {e}").into()))?;
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
    use crate::store::contract;

    async fn fresh_store(tag: &str) -> TursoStore {
        let path =
            std::env::temp_dir().join(format!("re_fw_store_test_{}_{tag}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = turso::Builder::new_local(path.to_str().expect("utf8 temp path"))
            .build()
            .await
            .expect("open test db");
        create_tables(&db).await.expect("create tables");
        TursoStore { db }
    }

    #[tokio::test]
    async fn cas_roundtrip_and_conflict() {
        contract::cas_roundtrip_and_conflict(&fresh_store("cas").await).await;
    }

    #[tokio::test]
    async fn outbox_lifecycle_and_dedup() {
        contract::outbox_lifecycle_and_dedup(&fresh_store("outbox").await).await;
    }

    #[tokio::test]
    async fn generation_guards() {
        contract::generation_guards(&fresh_store("gen").await).await;
    }

    #[tokio::test]
    async fn timer_deadlines() {
        contract::timer_deadlines(&fresh_store("timers").await).await;
    }
}
