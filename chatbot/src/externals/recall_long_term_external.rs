use std::sync::Arc;

use arrow_array::StringArray;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use lancedb::{query::ExecutableQuery, DistanceType};
use serenity::futures::{StreamExt, TryStreamExt};

use crate::{
    models::user::{HistoryEntry, UserAction},
    Env,
};

async fn recall(env: Arc<Env>, user_id: String, search_term: String) -> anyhow::Result<String> {
    let mut options = InitOptions::default();
    options.show_download_progress = true;
    options.model_name = EmbeddingModel::BGESmallENV15;
    let options = options;

    let mut model = TextEmbedding::try_new(options)
        .map_err(|e| e.to_string())
        .expect("Should be comile time");

    let query_embedding = model
        .embed(vec![search_term], None)
        .map_err(|e| e.to_string())
        .expect("Should be compile time")[0]
        .clone();

    let mut res = env
        .lance_service
        .history_table
        .query()
        .nearest_to(query_embedding)?
        .column("embedding")
        .execute()
        .await?;

    let mut buf = String::new();
    while let Some(batch) = res.try_next().await? {
        buf.push_str(&format!(
            "{}\n",
            batch
                .column_by_name("content")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0)
        ));
    }

    Ok(buf)
}

pub async fn execute_long_recall(
    env: Arc<Env>,
    user_id: String,
    search_term: String,
) -> UserAction {
    let result = recall(env, user_id, search_term)
        .await
        .map_err(|e| e.to_string());
    UserAction::InternalFunctionResult(result)
}
