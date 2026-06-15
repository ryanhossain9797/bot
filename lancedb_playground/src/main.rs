use std::sync::Arc;

use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, RecordBatchReader,
    StringArray,
};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use futures::TryStreamExt;
use lancedb::arrow::arrow_schema::{DataType, Field, Schema};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Table};

const DB_PATH: &str = "/tmp/lancedb_playground_db";

// Improvement 2: compare a stronger/bigger embedder against the bot's current BGE-small.
struct Embedder {
    label: &'static str,
    model: EmbeddingModel,
    dim: i32,
}

const EMBEDDERS: &[Embedder] = &[
    Embedder {
        label: "bge-small (384d, current)",
        model: EmbeddingModel::BGESmallENV15,
        dim: 384,
    },
    Embedder {
        label: "bge-base (768d, improvement 2)",
        model: EmbeddingModel::BGEBaseENV15,
        dim: 768,
    },
];

const QUERIES: &[&str] = &[
    "what is the user's name?",
    "what hardware does the user run the bot on?",
    "does the user prefer coffee or tea?",
    "how does the user deploy the bot?",
    "what is the user building?",
    "what is the user's favorite color?",
];

// Baseline: raw conversation turns, stored exactly as the bot's commit() formats them.
fn raw_turns() -> Vec<String> {
    [
        "USER MESSAGE: Zireael9797 (id:404941793076051968): hey, my name is Raiyan — remember that",
        "MESSAGE USER: Got it, Raiyan — noted.",
        "USER MESSAGE: Zireael9797 (id:404941793076051968): I'm building a Discord bot in Rust that runs a local LLM",
        "MESSAGE USER: Nice — a local-LLM Discord bot in Rust. What are you running it on?",
        "USER MESSAGE: Zireael9797 (id:404941793076051968): Qwen 27B on a Strix Halo with 96GB of unified memory",
        "MESSAGE USER: A Strix Halo with 96GB is a solid fit for a 27B model.",
        "USER MESSAGE: Zireael9797 (id:404941793076051968): what's the capital of Bangladesh?",
        "MESSAGE USER: The capital of Bangladesh is Dhaka.",
        "USER MESSAGE: Zireael9797 (id:404941793076051968): just so you know, I prefer coffee over tea",
        "MESSAGE USER: Noted — coffee over tea.",
        "USER MESSAGE: Zireael9797 (id:404941793076051968): I deploy the bot with `just deploy_local`",
        "MESSAGE USER: Right — `just deploy_local` builds the image and restarts the container.",
        "USER MESSAGE: Zireael9797 (id:404941793076051968): my favorite anime is Frieren",
        "MESSAGE USER: Frieren is a great pick.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

// Improvement 1: strip the "USER MESSAGE: Name (id:N): " / "MESSAGE USER: " boilerplate.
fn strip_prefix(s: &str) -> String {
    let body = s
        .strip_prefix("USER MESSAGE: ")
        .or_else(|| s.strip_prefix("MESSAGE USER: "))
        .unwrap_or(s);
    if let Some(pos) = body.find("): ") {
        if body[..pos].contains("(id:") {
            return body[pos + 3..].to_string();
        }
    }
    body.to_string()
}

// Improvement 3: distilled atomic facts — what an extraction pass would produce at commit.
fn distilled_facts() -> Vec<String> {
    [
        "The user's name is Raiyan.",
        "The user is building a Discord bot in Rust that runs a local LLM.",
        "The user runs Qwen 27B on a Strix Halo with 96GB of unified memory.",
        "The user prefers coffee over tea.",
        "The user deploys the bot with `just deploy_local`.",
        "The user's favorite anime is Frieren.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn memory_schema(dim: i32) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("content", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, false)), dim),
            false,
        ),
    ]))
}

fn make_model(model_name: EmbeddingModel) -> anyhow::Result<TextEmbedding> {
    let mut options = InitOptions::default();
    options.model_name = model_name;
    Ok(TextEmbedding::try_new(options)?)
}

fn make_batch(
    schema: Arc<Schema>,
    dim: i32,
    contents: Vec<String>,
    embeddings: Vec<Vec<f32>>,
) -> anyhow::Result<RecordBatch> {
    let flat: Vec<f32> = embeddings.into_iter().flatten().collect();
    let vectors = FixedSizeListArray::try_new(
        Arc::new(Field::new("item", DataType::Float32, false)),
        dim,
        Arc::new(Float32Array::from_iter_values(flat)),
        None,
    )?;
    Ok(RecordBatch::try_new(
        schema,
        vec![Arc::new(StringArray::from(contents)), Arc::new(vectors)],
    )?)
}

async fn build_table(
    conn: &lancedb::Connection,
    schema: Arc<Schema>,
    dim: i32,
    model: &mut TextEmbedding,
    name: &str,
    contents: Vec<String>,
) -> anyhow::Result<Table> {
    let table = conn
        .create_empty_table(name, Arc::clone(&schema))
        .execute()
        .await?;
    let embeddings = model.embed(contents.clone(), None)?;
    let batch = make_batch(Arc::clone(&schema), dim, contents, embeddings)?;
    let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
    table
        .add(Box::new(reader) as Box<dyn RecordBatchReader + Send>)
        .execute()
        .await?;
    Ok(table)
}

async fn top_hit(table: &Table, query_vec: Vec<f32>) -> anyhow::Result<(String, f32)> {
    let mut stream = table
        .query()
        .nearest_to(query_vec)?
        .column("embedding")
        .limit(1)
        .execute()
        .await?;
    while let Some(batch) = stream.try_next().await? {
        let content = batch
            .column_by_name("content")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("content column missing"))?;
        let distance = batch
            .column_by_name("_distance")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
        if batch.num_rows() > 0 {
            let d = distance.map(|a| a.value(0)).unwrap_or(f32::NAN);
            return Ok((content.value(0).to_string(), d));
        }
    }
    Ok(("<no hit>".to_string(), f32::NAN))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

async fn run_for_embedder(embedder: &Embedder) -> anyhow::Result<()> {
    let db_path = format!("{DB_PATH}_{}", embedder.dim);
    let _ = std::fs::remove_dir_all(&db_path);
    let mut model = make_model(embedder.model.clone())?;
    let schema = memory_schema(embedder.dim);
    let conn = connect(&db_path).execute().await?;

    let raw = raw_turns();
    let stripped: Vec<String> = raw.iter().map(|s| strip_prefix(s)).collect();
    let facts = distilled_facts();

    let t_raw = build_table(&conn, Arc::clone(&schema), embedder.dim, &mut model, "raw", raw).await?;
    let t_strip =
        build_table(&conn, Arc::clone(&schema), embedder.dim, &mut model, "strip", stripped).await?;
    let t_facts =
        build_table(&conn, Arc::clone(&schema), embedder.dim, &mut model, "facts", facts).await?;
    let tables: [(&str, &Table); 3] =
        [("raw  ", &t_raw), ("strip", &t_strip), ("facts", &t_facts)];

    println!("\n################################################################");
    println!("### {}", embedder.label);
    println!("################################################################");

    for q in QUERIES {
        println!("Q: {q:?}");
        let qv = model.embed(vec![q.to_string()], None)?.remove(0);
        for (label, table) in &tables {
            let (content, dist) = top_hit(table, qv.clone()).await?;
            println!("  [{label}] {dist:.3}  {}", truncate(&content, 68));
        }
        println!();
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== recall comparison — top-1 per design, lower distance = closer ===");
    println!("  raw   = conversation turns as the bot stores them now");
    println!("  strip = improvement 1: boilerplate prefix removed before embedding");
    println!("  facts = improvement 3: distilled atomic facts");
    println!("  (each embedder = improvement 2: same corpora, different model)");

    for embedder in EMBEDDERS {
        run_for_embedder(embedder).await?;
    }
    Ok(())
}
