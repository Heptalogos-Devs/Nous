use crate::{Database, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection};
impl Database {
    pub async fn formation(&self, id: &str) -> Result<FormationRun> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = read(&mut tx, id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
impl WriteTransaction {
    pub async fn formation(&mut self, id: &str) -> Result<FormationRun> {
        read(&mut self.tx, id).await
    }
    pub async fn insert_formation(&mut self, value: &FormationRun) -> Result<()> {
        let job = self.require_subject_job(&value.job).await?;
        let (namespace, subject) = job.subject_scope()?;
        let observation = self.observation(namespace, &value.observation_id).await?;
        let material = self.material(namespace, &value.material_id).await?;
        if observation.subject_id != *subject
            || observation.material_id != material.id
            || material.content_hash != value.material_hash
        {
            return Err(Error::invalid("formation source is outside its job"));
        }
        sqlx::query("INSERT INTO formation_runs(job_id,observation_id,material_id,subject_revision,material_hash) VALUES(?,?,?,?,?)").bind(&job.id).bind(value.observation_id.as_str()).bind(value.material_id.as_str()).bind(value.subject_revision).bind(&value.material_hash).execute(&mut *self.tx).await.map_err(write_error)?;
        self.replace_memories(value).await
    }
    pub async fn update_formation(&mut self, value: &FormationRun) -> Result<()> {
        let old = self.formation(&value.job.id).await?;
        if old.observation_id != value.observation_id
            || old.material_id != value.material_id
            || old.subject_revision != value.subject_revision
            || old.material_hash != value.material_hash
        {
            return Err(Error::new(ErrorCode::Conflict, "formation input is frozen"));
        }
        self.replace_memories(value).await
    }
    async fn replace_memories(&mut self, value: &FormationRun) -> Result<()> {
        let (namespace, subject) = value.job.subject_scope()?;
        sqlx::query("DELETE FROM formation_memories WHERE job_id=?")
            .bind(&value.job.id)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        for (ordinal, id) in value.memory_ids.iter().enumerate() {
            let memory = self.memory(namespace, id).await?;
            if memory.subject_id != *subject {
                return Err(Error::invalid("formation result is outside its subject"));
            }
            sqlx::query("INSERT INTO formation_memories(job_id,memory_id,ordinal) VALUES(?,?,?)")
                .bind(&value.job.id)
                .bind(id.as_str())
                .bind(ordinal as i64)
                .execute(&mut *self.tx)
                .await
                .map_err(write_error)?;
        }
        Ok(())
    }
}
async fn read(conn: &mut SqliteConnection, id: &str) -> Result<FormationRun> {
    let row = sqlx::query("SELECT * FROM formation_runs WHERE job_id=?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT memory_id FROM formation_memories WHERE job_id=? ORDER BY ordinal",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await
    .map_err(storage_error)?;
    Ok(FormationRun {
        job: crate::authority::tasks::read(conn, id).await?,
        observation_id: ObservationId(row.get("observation_id")),
        material_id: MaterialId(row.get("material_id")),
        subject_revision: row.get("subject_revision"),
        material_hash: row.get("material_hash"),
        memory_ids: ids.into_iter().map(MemoryId).collect(),
    })
}
