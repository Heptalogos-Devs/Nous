use crate::{Database, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqliteConnection;
mod groups;
pub use groups::{StructuredFormationGroup, StructuredGroupState};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredFormationRun {
    pub job: JobRecord,
    pub input: Value,
}
impl StructuredFormationRun {
    pub fn is_retired(&self) -> bool {
        self.job.state == JobState::Cancelled
            || self.input.get("redacted") == Some(&Value::Bool(true))
    }
}
impl Database {
    pub async fn structured_formation(&self, id: &str) -> Result<StructuredFormationRun> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = read(&mut tx, id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
impl WriteTransaction {
    pub async fn structured_formation(&mut self, id: &str) -> Result<StructuredFormationRun> {
        read(&mut self.tx, id).await
    }
    pub async fn insert_structured_formation(
        &mut self,
        value: &StructuredFormationRun,
    ) -> Result<()> {
        self.require_subject_job(&value.job).await?;
        if !value.input.is_object() {
            return Err(Error::invalid("formation input must be an object"));
        }
        self.validate_structured_formation_sources(value).await?;
        sqlx::query("INSERT INTO structured_formation_runs(job_id,input) VALUES(?,?)")
            .bind(&value.job.id)
            .bind(serde_json::to_string(&value.input)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
}
async fn read(conn: &mut SqliteConnection, id: &str) -> Result<StructuredFormationRun> {
    let input: String =
        sqlx::query_scalar("SELECT input FROM structured_formation_runs WHERE job_id=?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(storage_error)?
            .ok_or_else(not_found)?;
    Ok(StructuredFormationRun {
        job: crate::authority::tasks::read(conn, id).await?,
        input: serde_json::from_str(&input)?,
    })
}
