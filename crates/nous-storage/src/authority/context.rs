use crate::{ReadView, WriteTransaction, not_found, storage_error};
use nous_domain::{CallContext, Result};
use sqlx::SqliteConnection;

async fn accepted(conn: &mut SqliteConnection, commit: i64) -> Result<CallContext> {
    let context: String = sqlx::query_scalar(
        "SELECT json_extract(receipt,'$.context') FROM operation_receipts WHERE commit_seq=?",
    )
    .bind(commit)
    .fetch_optional(conn)
    .await
    .map_err(storage_error)?
    .ok_or_else(not_found)?;
    Ok(serde_json::from_str(&context)?)
}
macro_rules! accepted_context {
    ($owner:ty) => {
        impl $owner {
            pub async fn accepted_context(&mut self, commit_seq: i64) -> Result<CallContext> {
                accepted(&mut self.tx, commit_seq).await
            }
        }
    };
}
accepted_context!(ReadView);
accepted_context!(WriteTransaction);
