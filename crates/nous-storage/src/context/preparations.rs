use super::*;
use crate::{Database, ReadView, WriteTransaction, write_error};

async fn preparation(
    conn: &mut SqliteConnection,
    id: &ContextPreparationId,
    job: JobRecord,
) -> Result<ContextPreparation> {
    let row = sqlx::query("SELECT * FROM context_preparations WHERE id=?")
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    Ok(ContextPreparation {
        id: id.clone(),
        job,
        request: decode(row.get("request"))?,
        config_digest: row.get("config_digest"),
        policy: optional(row.get("policy"))?,
        catalog: decode(row.get("catalog"))?,
        work_context: optional(row.get("work_context"))?,
        session_revision: row.get("session_revision"),
        source_watermark: row.get("source_watermark"),
        steward_job_id: row.get("steward_job_id"),
        response: optional(row.get("response"))?,
        degradation: decode(row.get("degradation"))?,
    })
}
fn optional<T: serde::de::DeserializeOwned>(body: Option<&str>) -> Result<Option<T>> {
    body.map(decode).transpose()
}
macro_rules! read_preparation {
    ($owner:ty)=>{impl $owner{
        pub async fn context_preparation(&mut self,id:&ContextPreparationId)->Result<ContextPreparation>{
            let job:String=sqlx::query_scalar("SELECT job_id FROM context_preparations WHERE id=?").bind(id.as_str()).fetch_optional(&mut *self.tx).await.map_err(storage_error)?.ok_or_else(not_found)?;
            let job=self.job_record(&job).await?; preparation(&mut self.tx,id,job).await
        }
        pub async fn context_preparation_by_job(&mut self,job_id:&str)->Result<ContextPreparation>{
            let id:String=sqlx::query_scalar("SELECT id FROM context_preparations WHERE job_id=? OR id=(SELECT preparation_id FROM context_steward_jobs WHERE job_id=?)").bind(job_id).bind(job_id).fetch_optional(&mut *self.tx).await.map_err(storage_error)?.ok_or_else(not_found)?;
            self.context_preparation(&ContextPreparationId(id)).await
        }
    }};
}
read_preparation!(ReadView);
read_preparation!(WriteTransaction);
impl Database {
    pub async fn context_preparation(
        &self,
        id: &ContextPreparationId,
    ) -> Result<ContextPreparation> {
        let mut view = self.read_view().await?;
        let value = view.context_preparation(id).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn context_preparation_by_job(&self, id: &str) -> Result<ContextPreparation> {
        let mut view = self.read_view().await?;
        let value = view.context_preparation_by_job(id).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn context_projection(&self, id: &ContextProjectionId) -> Result<ContextProjection> {
        projection(&mut *self.pool.acquire().await.map_err(storage_error)?, id).await
    }
}
impl WriteTransaction {
    pub async fn insert_context_preparation(&mut self, value: &ContextPreparation) -> Result<()> {
        value.request.validate()?;
        let (ns, subject) = value.job.subject_scope()?;
        self.context().require_subject(ns, subject)?;
        if subject != &value.request.subject_id
            || value.response.is_some()
            || value.steward_job_id.is_some()
        {
            return Err(Error::invalid(
                "context preparation must begin with its accepted scoped input",
            ));
        }
        let session = session(&mut self.tx, &value.request.session_id).await?;
        if &session.namespace_id != ns
            || &session.subject_id != subject
            || session.closed_at.is_some()
            || session.history_invalidated
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "context Session needs refresh before preparation",
            ));
        }
        if value.session_revision != Some(session.revision) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "context Session changed during source collection",
            ));
        }
        if let Some(context) = &value.work_context {
            let head = work(&mut self.tx, &context.id).await?;
            if head.revision != context.revision
                || head.state != WorkContextState::Open
                || session.active_work_context.as_ref() != Some(&context.id)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "foreground WorkContext changed before context acceptance",
                ));
            }
        }
        sqlx::query("INSERT INTO context_preparations(id,job_id,request,config_digest,policy,catalog,work_context,session_revision,source_watermark,degradation) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str())
            .bind(&value.job.id)
            .bind(encode(&value.request)?)
            .bind(&value.config_digest)
            .bind(value.policy.as_ref().map(encode).transpose()?)
            .bind(encode(&value.catalog)?)
            .bind(value.work_context.as_ref().map(encode).transpose()?)
            .bind(value.session_revision)
            .bind(value.source_watermark)
            .bind(encode(&value.degradation)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn update_context_preparation(&mut self, value: &ContextPreparation) -> Result<()> {
        let current = self.context_preparation(&value.id).await?;
        let (ns, subject) = value.job.subject_scope()?;
        self.context().require_subject(ns, subject)?;
        if encode(&current.request)? != encode(&value.request)?
            || (current.config_digest.is_some()
                && (current.config_digest != value.config_digest
                    || encode(&current.catalog)? != encode(&value.catalog)?))
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "prepared context input snapshot is immutable",
            ));
        }
        if let Some(response) = current.response {
            if encode(&response)? != encode(&value.response)? {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "context response is already compiled",
                ));
            }
        }
        sqlx::query("UPDATE context_preparations SET config_digest=?,policy=?,catalog=?,work_context=?,session_revision=?,source_watermark=?,steward_job_id=?,response=?,degradation=? WHERE id=?")
            .bind(&value.config_digest).bind(value.policy.as_ref().map(encode).transpose()?).bind(encode(&value.catalog)?).bind(value.work_context.as_ref().map(encode).transpose()?)
            .bind(value.session_revision).bind(value.source_watermark).bind(&value.steward_job_id).bind(value.response.as_ref().map(encode).transpose()?).bind(encode(&value.degradation)?).bind(value.id.as_str()).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub async fn insert_context_steward(
        &mut self,
        id: &ContextPreparationId,
        job: &JobRecord,
    ) -> Result<()> {
        let preparation = self.context_preparation(id).await?;
        if job.namespace_id != preparation.job.namespace_id
            || job.subject_id != preparation.job.subject_id
        {
            return Err(Error::invalid(
                "Steward job belongs to another context scope",
            ));
        }
        sqlx::query("INSERT INTO context_steward_jobs(job_id,preparation_id) VALUES(?,?)")
            .bind(&job.id)
            .bind(id.as_str())
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn context_projection(
        &mut self,
        id: &ContextProjectionId,
    ) -> Result<ContextProjection> {
        projection(&mut self.tx, id).await
    }
    pub async fn save_context_projection(
        &mut self,
        preparation: &ContextPreparationId,
        value: &ContextProjection,
    ) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        self.context()
            .require_subject(&value.namespace_id, &value.subject_id)?;
        let session = session(&mut self.tx, &value.session_id).await?;
        if session.namespace_id != value.namespace_id
            || session.subject_id != value.subject_id
            || session.revision != value.session_revision
            || session.history_invalidated
            || session.closed_at.is_some()
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "context Session changed during preparation",
            ));
        }
        if let Some(context) = &value.work_context {
            let head = work(&mut self.tx, &context.id).await?;
            if head.revision != context.revision || head.state != WorkContextState::Open {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "foreground WorkContext changed during preparation",
                ));
            }
        }
        sqlx::query("INSERT INTO context_projections(id,namespace_id,subject_id,session_id,preparation_id,body,commit_seq) VALUES(?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.session_id.as_str()).bind(preparation.as_str()).bind(encode(value)?).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
async fn projection(
    conn: &mut SqliteConnection,
    id: &ContextProjectionId,
) -> Result<ContextProjection> {
    let body: String = sqlx::query_scalar("SELECT body FROM context_projections WHERE id=?")
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    decode(&body)
}
