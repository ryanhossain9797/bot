use crate::{
    types::conversation::{HistoryEntry, LLMInput, LLMResponse, ConversationAction},
    services::lance_db::LanceService,
    Env,
};
use arrow_array::{
    FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, RecordBatchReader,
    StringArray,
};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use lancedb::{
    arrow::arrow_schema::{DataType, Field},
    index::{vector::IvfFlatIndexBuilder, Index},
    Table,
};
use std::sync::Arc;

pub async fn ensure_embedding_index(table: &Table, column: &str) -> Result<(), String> {
    let existing_indexes = table.list_indices().await.map_err(|e| e.to_string())?;

    let index_exists = existing_indexes
        .iter()
        .any(|idx| idx.columns.contains(&column.to_string()));

    if !index_exists {
        println!("Creating vector index on column '{}'", column);

        table
            .create_index(&[column], Index::IvfFlat(IvfFlatIndexBuilder::default()))
            .execute()
            .await
            .map_err(|e| e.to_string())?;

        println!("Index created!");
    } else {
        println!("Index already exists, skipping creation");
    }

    Ok(())
}

async fn commit(
    lance_service: Arc<LanceService>,
    conversation_id: String,
    history: Vec<HistoryEntry>,
) -> Result<(), String> {
    let schema = Arc::clone(&lance_service.history_schema);

    let table = lance_service.table_for_conversation(&conversation_id).await;

    let filtered: Vec<String> = history
        .iter()
        .filter_map(|h| match h {
            HistoryEntry::Input(LLMInput::ConversationMessage(msg)) => {
                Some(format!("USER MESSAGE: {}", msg.text))
            }
            HistoryEntry::Output(LLMResponse {
                message: Some(response),
                ..
            }) => Some(format!("MESSAGE USER: {response}")),
            _ => None,
        })
        .collect();

    if filtered.is_empty() {
        return Ok(());
    }

    let mut options = InitOptions::default();
    options.show_download_progress = true;
    options.model_name = EmbeddingModel::BGESmallENV15;
    let options = options;

    let mut model = TextEmbedding::try_new(options).map_err(|e| e.to_string())?;

    println!("Generating embeddings for {} entries", filtered.len());

    let embeddings = model
        .embed(filtered.clone(), None)
        .map_err(|e| e.to_string())?;

    let vector_dim = embeddings[0].len();
    let flat_embeddings: Vec<f32> = embeddings.into_iter().flatten().collect();

    let values = Float32Array::from_iter_values(flat_embeddings);

    let vector_array = FixedSizeListArray::try_new(
        Arc::new(Field::new("item", DataType::Float32, false)),
        vector_dim as i32,
        Arc::new(values),
        None,
    )
    .map_err(|e| e.to_string())?;

    let conversation_ids: Vec<String> = vec![conversation_id.clone(); filtered.len()];

    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(StringArray::from(conversation_ids)),
            Arc::new(StringArray::from(filtered)),
            Arc::new(vector_array),
        ],
    )
    .map_err(|e| e.to_string())?;

    let reader = RecordBatchIterator::new(vec![Ok(batch)], Arc::clone(&schema));

    table
        .add(Box::new(reader) as Box<dyn RecordBatchReader + Send>)
        .execute()
        .await
        .map_err(|e| e.to_string())?;

    ensure_embedding_index(&table, "embedding").await?;

    Ok(())
}

pub async fn commit_to_memory(
    env: Arc<Env>,
    conversation_id: String,
    history: Vec<HistoryEntry>,
) -> ConversationAction {
    let result = commit(Arc::clone(&env.lance_service), conversation_id, history).await;
    if let Err(err) = &result {
        eprintln!("[memory] commit failed: {err}");
    }
    ConversationAction::CommitResult(result)
}
