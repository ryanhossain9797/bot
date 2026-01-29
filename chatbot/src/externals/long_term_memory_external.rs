use crate::{
    models::user::{HistoryEntry, LLMDecisionType, LLMInput, LLMResponse, UserAction},
    services::lance_db::LanceService,
    Env,
};
use arrow_array::{
    FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use lancedb::{
    arrow::arrow_schema::{DataType, Field},
    index::{vector::IvfFlatIndexBuilder, Index},
    Table,
};
use std::sync::Arc;

pub async fn ensure_embedding_index(table: &Table, column: &str) -> Result<(), String> {
    // Check if index already exists
    let existing_indexes = table.list_indices().await.map_err(|e| e.to_string())?;

    let index_exists = existing_indexes
        .iter()
        .any(|idx| idx.columns.contains(&column.to_string()));

    if !index_exists {
        println!("Creating vector index on column '{}'", column);

        // Build HNSW index (fast and accurate for small-to-medium datasets)
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
    user_id: String,
    history: Vec<HistoryEntry>,
) -> Result<(), String> {
    let schema = Arc::clone(&lance_service.history_schema);

    let table = lance_service.table_for_user(&user_id).await;

    let filtered: Vec<String> = history
        .iter()
        .filter_map(|h| match h {
            HistoryEntry::Input(LLMInput::UserMessage(text)) => {
                Some(format!("USER MESSAGE: {text}"))
            }
            HistoryEntry::Output(LLMResponse {
                outcome: LLMDecisionType::MessageUser { response },
                ..
            }) => Some(format!("MESSAGE USER: {response}")),
            // ... other mappings
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

    let vector_dim = embeddings[0].len(); // Usually 384 for BGE-Small
    let flat_embeddings: Vec<f32> = embeddings.into_iter().flatten().collect();

    let values = Float32Array::from_iter_values(flat_embeddings);

    let vector_array = FixedSizeListArray::try_new(
        Arc::new(Field::new("item", DataType::Float32, false)),
        vector_dim as i32,
        Arc::new(values),
        None, // No null bitmap
    )
    .map_err(|e| e.to_string())?;

    let user_ids: Vec<String> = vec![user_id.clone(); filtered.len()];

    // 4. Build RecordBatch (Ensure your schema matches these 3 columns)
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(StringArray::from(user_ids)),
            Arc::new(StringArray::from(filtered)),
            Arc::new(vector_array), // The new vector column
        ],
    )
    .map_err(|e| e.to_string())?;

    let reader = RecordBatchIterator::new(vec![Ok(batch)], Arc::clone(&schema));

    table
        .add(reader)
        .execute()
        .await
        .map_err(|e| e.to_string())?;

    ensure_embedding_index(&table, "embedding").await?;

    Ok(())
}

pub async fn commit_to_memory(
    env: Arc<Env>,
    user_id: String,
    history: Vec<HistoryEntry>,
) -> UserAction {
    UserAction::CommitResult(commit(Arc::clone(&env.lance_service), user_id, history).await)
}
