use crate::{SqliteStorage, StorageError};
use music_application::assistant::{AssistantFuture, ModelBatchRecord, ModelBatchRepository};
use sqlx::Row;

fn decode(
    row: sqlx::sqlite::SqliteRow,
) -> Result<ModelBatchRecord, Box<dyn std::error::Error + Send + Sync>> {
    Ok(ModelBatchRecord {
        id: row.try_get("id")?,
        connection_id: row.try_get("connection_id")?,
        state: row.try_get("state")?,
        input_file_id: row.try_get("input_file_id")?,
        remote_batch_id: row.try_get("remote_batch_id")?,
        document: serde_json::from_str(&row.try_get::<String, _>("document_json")?)?,
    })
}

impl ModelBatchRepository for SqliteStorage {
    fn model_batch(&self, id: &str) -> AssistantFuture<'_, Option<ModelBatchRecord>> {
        let id = id.to_owned();
        Box::pin(async move {
            sqlx::query("SELECT * FROM assistant_model_batches WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .map(decode)
                .transpose()
        })
    }
    fn model_batch_summary(&self, id: &str) -> AssistantFuture<'_, Option<ModelBatchRecord>> {
        let id = id.to_owned();
        Box::pin(async move {
            sqlx::query("SELECT id, connection_id, state, input_file_id, remote_batch_id, json_object('result',json_extract(document_json,'$.result')) AS document_json FROM assistant_model_batches WHERE id = ?")
                .bind(id).fetch_optional(&self.pool).await?.map(decode).transpose()
        })
    }
    fn pending_model_batch(&self) -> AssistantFuture<'_, Option<ModelBatchRecord>> {
        Box::pin(async move {
            sqlx::query("SELECT id, connection_id, state, input_file_id, remote_batch_id, 'null' AS document_json FROM assistant_model_batches WHERE state NOT IN ('completed','cancelled','failed','expired') ORDER BY rowid LIMIT 1")
                .fetch_optional(&self.pool).await?.map(decode).transpose()
        })
    }
    fn create_model_batch<'a>(&'a self, record: &'a ModelBatchRecord) -> AssistantFuture<'a, bool> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            if record.document.to_string().len() > 64 * 1024 * 1024 {
                return Err(Box::new(StorageError::InvalidOption(
                    "model batch document too large",
                )) as _);
            }
            if self.pending_model_batch().await?.is_some() {
                return Ok(false);
            }
            sqlx::query("INSERT INTO assistant_model_batches (id,connection_id,state,document_json) VALUES (?,?,?,?)")
                .bind(&record.id).bind(&record.connection_id).bind(&record.state)
                .bind(record.document.to_string()).execute(&self.pool).await?;
            Ok(true)
        })
    }
    fn update_model_batch<'a>(
        &'a self,
        expected: &'a str,
        record: &'a ModelBatchRecord,
    ) -> AssistantFuture<'a, bool> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            if record.document.to_string().len() > 64 * 1024 * 1024 {
                return Err(Box::new(StorageError::InvalidOption(
                    "model batch document too large",
                )) as _);
            }
            Ok(sqlx::query("UPDATE assistant_model_batches SET state=?,input_file_id=?,remote_batch_id=?,document_json=?,updated_at=CURRENT_TIMESTAMP WHERE id=? AND connection_id=? AND state=?")
                .bind(&record.state).bind(&record.input_file_id).bind(&record.remote_batch_id)
                .bind(record.document.to_string()).bind(&record.id).bind(&record.connection_id).bind(expected)
                .execute(&self.pool).await?.rows_affected() == 1)
        })
    }
}
