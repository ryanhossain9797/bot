use crate::{
    models::user::{HistoryEntry, UserAction},
    services::lance_db::LanceService,
    Env,
};

use arrow_array::{RecordBatch, RecordBatchIterator, StringArray};
use lancedb::{
    arrow::arrow_schema::{DataType, Field, Schema},
    connect, Table,
};
use std::sync::Arc;

async fn commit(
    lance_service: Arc<LanceService>,
    user_id: String,
    history: Vec<HistoryEntry>,
) -> Result<(), String> {
    let (schema, table) = (
        Arc::clone(&lance_service.history_schema),
        &lance_service.history_table,
    );

    let user_ids: Vec<&str> = history.iter().map(|_| user_id.as_str()).collect();
    let contents: Vec<&str> = history.iter().map(|h| "PLACEHOLDER").collect();

    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(StringArray::from(user_ids)),
            Arc::new(StringArray::from(contents)),
        ],
    )
    .map_err(|e| e.to_string())?;

    println!("Data Batch Ready");

    let reader = RecordBatchIterator::new(vec![Ok(batch)], Arc::clone(&schema));

    table
        .add(reader)
        .execute()
        .await
        .map_err(|e| e.to_string())?;

    println!("Inserted");

    Ok(())
}

pub async fn commit_to_memory(
    env: Arc<Env>,
    user_id: String,
    history: Vec<HistoryEntry>,
) -> UserAction {
    UserAction::CommitResult(commit(Arc::clone(&env.lance_service), user_id, history).await)
}
