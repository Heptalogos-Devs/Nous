use crate::{JOB_QUEUE, QueueRecord, ReadView, storage_error};
use nous_domain::Result;
use sqlx::{Row, SqliteConnection};

pub(crate) async fn queue_record(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<QueueRecord>> {
    Ok(sqlx::query(
        "SELECT id,status,attempts,max_attempts,last_result FROM Jobs WHERE id=? AND job_type=?",
    )
    .bind(id)
    .bind(JOB_QUEUE)
    .fetch_optional(conn)
    .await
    .map_err(storage_error)?
    .map(|row| QueueRecord {
        id: row.get("id"),
        status: row.get("status"),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
        last_result: row.get("last_result"),
    }))
}
impl ReadView {
    pub async fn queue_record(&mut self, id: &str) -> Result<Option<QueueRecord>> {
        queue_record(&mut self.tx, id).await
    }
}
