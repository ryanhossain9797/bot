use std::sync::Arc;

use lancedb::{
    arrow::arrow_schema::{DataType, Field, Schema},
    connect, Connection, Table,
};

pub struct LanceService {
    connection: Connection,
    pub history_schema: Arc<Schema>,
}

impl LanceService {
    pub async fn new() -> Self {
        let (connection, history_schema) = setup_table_and_schema().await;

        LanceService {
            connection,
            history_schema,
        }
    }

    pub async fn table_for_user(&self, user_id: &str) -> Table {
        let table_name = format!("history_{}", user_id);

        match self.connection.open_table(&table_name).execute().await {
            Ok(t) => t,
            Err(_) => self
                .connection
                .create_empty_table(&table_name, Arc::clone(&self.history_schema))
                .execute()
                .await
                .map_err(|e| e.to_string())
                .expect("Edge cases should be handled at compile time"),
        }
    }
}

async fn setup_table_and_schema() -> (Connection, Arc<Schema>) {
    let db_conn = connect("long_term/memory")
        .execute()
        .await
        .map_err(|e| e.to_string())
        .expect("Edge cases should be handled at compile time");

    println!("Connected");

    let dim = 384;
    let schema = Arc::new(Schema::new(vec![
        Field::new("user_id", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, false)), dim),
            false,
        ),
    ]));

    println!("Schema Ready");

    // let table = match db.open_table("history").execute().await {
    //     Ok(t) => t,
    //     Err(_) => db
    //         .create_empty_table("history", schema.clone())
    //         .execute()
    //         .await
    //         .map_err(|e| e.to_string())
    //         .expect("Edge cases should be handled at compile time"),
    // };

    // println!("Table Ready");

    // (schema, table)
    (db_conn, schema)
}
