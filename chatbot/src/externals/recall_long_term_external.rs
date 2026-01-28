use std::sync::Arc;

use arrow_array::StringArray;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use lancedb::query::ExecutableQuery;
use serenity::futures::TryStreamExt;

use crate::{models::user::UserAction, Env};

async fn recall(env: Arc<Env>, user_id: String, search_term: String) -> anyhow::Result<String> {
    let mut options = InitOptions::default();
    options.show_download_progress = true;
    options.model_name = EmbeddingModel::BGESmallENV15;
    let options = options;

    let mut model = TextEmbedding::try_new(options)?;

    let query_embedding = model.embed(vec![search_term], None)?[0].clone();

    let history_table = env.lance_service.table_for_user(&user_id).await;

    let mut res = history_table
        .query()
        .nearest_to(query_embedding)?
        .column("embedding")
        .execute()
        .await?;

    let mut buf = String::new();
    while let Some(batch) = res.try_next().await? {
        let column = batch
            .column_by_name("content")
            .ok_or_else(|| anyhow::Error::msg("column 'content' missing".to_string()))?;

        // 2. Downcast
        let array = column
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                anyhow::Error::msg("column 'content' is not a StringArray".to_string())
            })?;

        // 3. Extract and push
        buf.push_str(array.value(0));
        buf.push('\n');
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
