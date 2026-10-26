mod feedback;
mod preparations;
mod residents;
mod retirement;
mod sessions;
mod work;
use crate::{not_found, storage_error};
use nous_domain::{context_work::*, *};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Row, SqliteConnection};

fn encode(value: &impl Serialize) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}
fn decode<T: DeserializeOwned>(body: &str) -> Result<T> {
    Ok(serde_json::from_str(body)?)
}
pub(crate) async fn work(conn: &mut SqliteConnection, id: &WorkContextId) -> Result<WorkContext> {
    let body: String = sqlx::query_scalar(
        "SELECT body FROM work_context_revisions WHERE id=? ORDER BY revision DESC LIMIT 1",
    )
    .bind(id.as_str())
    .fetch_optional(conn)
    .await
    .map_err(storage_error)?
    .ok_or_else(not_found)?;
    decode(&body)
}
async fn session(conn: &mut SqliteConnection, id: &ContextSessionId) -> Result<ContextSession> {
    let row = sqlx::query("SELECT body,history_invalidated FROM context_sessions WHERE id=?")
        .bind(id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    let mut value: ContextSession = decode(row.get("body"))?;
    value.history_invalidated = row.get("history_invalidated");
    value.residents = residents::read(conn, id, false).await?;
    Ok(value)
}
