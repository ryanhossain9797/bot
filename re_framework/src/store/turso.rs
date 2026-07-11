
use crate::store::{
    init_store, CallToken, LoadedEntity, OutboxDraft, OutboxRow, RowKind, SaveOutcome, Store, TransitionWrite,
};
use anyhow::Context;
use async_trait::async_trait;

pub(crate) struct TursoStore {
    db: turso::Database,
}

pub async fn init_turso_store(path: &str) -> anyhow::Result<()> {
    let backend = open_turso_store(path).await?;
    init_store(backend)
}

async fn open_turso_store(path: &str) -> anyhow::Result<TursoStore> {
    if let Some(dir) = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("create_dir_all {}", dir.display()))?;
    }
    let db = turso::Builder::new_local(path)
        .build()
        .await
        .with_context(|| format!("open turso db at {path}"))?;
    create_tables(&db).await?;
    Ok(TursoStore { db })
}

async fn create_tables(db: &turso::Database) -> anyhow::Result<()> {
    let conn = db.connect().context("connect for schema init")?;
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
    .context("create framework tables")
}

impl TursoStore {
    fn connect(&self) -> anyhow::Result<turso::Connection> {
        let conn = self.db.connect().context("turso connect")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("set busy_timeout")?;
        Ok(conn)
    }
}

#[async_trait]
impl Store for TursoStore {
    async fn load(&self, machine: &'static str, id_string: &str) -> anyhow::Result<Option<LoadedEntity>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT state, generation, version, next_outbox_seq FROM entities WHERE machine = ? AND id = ?",
                (machine, id_string),
            )
            .await
            .context("load entity")?;
        match rows.next().await.context("load entity row")? {
            None => Ok(None),
            Some(row) => Ok(Some(LoadedEntity {
                state_json: row.get(0).context("state column")?,
                generation: row.get(1).context("generation column")?,
                version: row.get(2).context("version column")?,
                next_outbox_seq: row.get(3).context("next_outbox_seq column")?,
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
    ) -> anyhow::Result<SaveOutcome> {
        let conn = self.connect()?;
        conn.execute("BEGIN IMMEDIATE", ()).await.context("begin insert")?;
        let result =
            insert_in_tx(&conn, machine, id_string, id_json, generation, state_json, next_tick_on, outbox).await;
        finish_tx(&conn, result).await
    }

    async fn save(&self, write: &TransitionWrite) -> anyhow::Result<SaveOutcome> {
        let conn = self.connect()?;
        conn.execute("BEGIN IMMEDIATE", ()).await.context("begin save")?;
        let result = save_in_tx(&conn, write).await;
        finish_tx(&conn, result).await
    }

    async fn is_duplicate(
        &self,
        machine: &'static str,
        id_string: &str,
        token: &CallToken,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT caller_generation, last_seq FROM call_dedup
                 WHERE machine = ? AND id = ? AND caller_machine = ? AND caller_id = ?",
                (machine, id_string, token.sender_machine, token.sender_id.as_str()),
            )
            .await
            .context("dedup lookup")?;
        match rows.next().await.context("dedup lookup row")? {
            None => Ok(false),
            Some(row) => {
                let slot_generation: i64 = row.get(0).context("caller_generation column")?;
                let last_seq: i64 = row.get(1).context("last_seq column")?;
                Ok(match slot_generation.cmp(&token.sender_generation) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Equal => last_seq >= token.seq,
                    std::cmp::Ordering::Less => false,
                })
            }
        }
    }

    async fn pending_outbox(&self, machine: &'static str, sender_id: &str) -> anyhow::Result<Vec<OutboxRow>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT seq, target_machine, target_id_json, action, kind, sender_generation FROM outbox
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
                sender_generation: row.get(5).context("sender_generation column")?,
                kind: RowKind::parse(&kind)
                    .with_context(|| format!("unknown outbox row kind {kind}"))?,
                target_machine: row.get(1).context("target_machine column")?,
                target_id_json: row.get(2).context("target_id_json column")?,
                payload_json: row.get(3).context("action column")?,
            });
        }
        Ok(pending)
    }

    async fn stalled_outbox_senders(
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
        collect_pairs(&mut rows).await
    }

    async fn due_timers(&self, cutoff_ms: i64, limit: i64, offset: i64) -> anyhow::Result<Vec<(String, String)>> {
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
        collect_pairs(&mut rows).await
    }

    async fn ack_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
        sender_generation: i64,
        seq: i64,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM outbox
             WHERE sender_machine = ? AND sender_id = ? AND sender_generation = ? AND seq = ?",
            (machine, sender_id, sender_generation, seq),
        )
        .await
        .context("ack outbox")?;
        Ok(())
    }

    async fn fail_outbox(
        &self,
        machine: &'static str,
        sender_id: &str,
        sender_generation: i64,
        seq: i64,
        reason: &str,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE outbox SET failure = ?
             WHERE sender_machine = ? AND sender_id = ? AND sender_generation = ? AND seq = ?",
            (reason, machine, sender_id, sender_generation, seq),
        )
        .await
        .context("fail outbox")?;
        Ok(())
    }

    async fn delete(&self, machine: &'static str, id_string: &str) -> anyhow::Result<()> {
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

async fn collect_pairs(rows: &mut turso::Rows) -> anyhow::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.context("pair row")? {
        out.push((row.get(0).context("column 0")?, row.get(1).context("column 1")?));
    }
    Ok(out)
}

async fn current_version(conn: &turso::Connection, machine: &str, id: &str) -> anyhow::Result<Option<i64>> {
    let mut rows = conn
        .query("SELECT version FROM entities WHERE machine = ? AND id = ?", (machine, id))
        .await
        .context("version probe")?;
    match rows.next().await.context("version probe row")? {
        Some(row) => Ok(Some(row.get(0).context("version column")?)),
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
) -> anyhow::Result<SaveOutcome> {
    if let Some(actual) = current_version(conn, machine, id_string).await? {
        return Ok(SaveOutcome::Conflict { actual: Some(actual) });
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
    .context("insert entity")?;
    insert_outbox_rows(conn, machine, id_string, id_json, generation, 0, outbox).await?;
    Ok(SaveOutcome::Ok)
}

async fn save_in_tx(conn: &turso::Connection, write: &TransitionWrite) -> anyhow::Result<SaveOutcome> {
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
        .context("CAS update")?;
    if updated == 0 {
        let actual = current_version(conn, write.machine, &write.id_string).await?;
        return Ok(SaveOutcome::Conflict { actual });
    }
    insert_outbox_rows(conn, write.machine, &write.id_string, &write.id_json, write.generation, write.first_seq, &write.outbox)
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
        .context("dedup upsert")?;
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
) -> anyhow::Result<()> {
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
    use crate::store::contract;

    async fn fresh_store(tag: &str) -> TursoStore {
        let path = std::env::temp_dir().join(format!("re_fw_store_test_{}_{tag}.db", std::process::id()));
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
