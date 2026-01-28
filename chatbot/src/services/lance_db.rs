use std::sync::Arc;

use lancedb::{
    arrow::arrow_schema::{DataType, Field, Schema},
    connect, Table,
};

pub struct LanceService {
    pub history_table: Table,
    pub history_schema: Arc<Schema>,
}

impl LanceService {
    pub async fn new() -> Self {
        let (history_schema, history_table) = setup_table_and_schema().await;

        LanceService {
            history_table,
            history_schema,
        }
    }
}

async fn setup_table_and_schema() -> (Arc<Schema>, Table) {
    let db = connect("long_term/memory")
        .execute()
        .await
        .map_err(|e| e.to_string())
        .expect("Edge cases should be handled at compile time");

    println!("Connected");

    let schema = Arc::new(Schema::new(vec![
        Field::new("user_id", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
    ]));

    println!("Schema");

    let table = match db.open_table("history").execute().await {
        Ok(t) => t,
        Err(_) => db
            .create_empty_table("history", schema.clone())
            .execute()
            .await
            .map_err(|e| e.to_string())
            .expect("Edge cases should be handled at compile time"),
    };

    println!("Table Ready");

    (schema, table)
}
