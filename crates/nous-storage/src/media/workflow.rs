use crate::{
    Database, ReadView, WriteTransaction, codec::*, not_found, storage_error, write_error,
};
use nous_domain::{media::*, *};
use sqlx::{Row, SqliteConnection};
impl Database {
    pub async fn media_derivation(&self, id: &str) -> Result<MediaDerivationRun> {
        let mut view = self.read_view().await?;
        let run = view.media_derivation(id).await?;
        view.finish().await?;
        Ok(run)
    }
}
impl ReadView {
    pub async fn media_derivation(&mut self, id: &str) -> Result<MediaDerivationRun> {
        let job = self.job_record(id).await?;
        read(&mut self.tx, job).await
    }
}
impl WriteTransaction {
    pub async fn media_derivation(&mut self, id: &str) -> Result<MediaDerivationRun> {
        let job = self.job_record(id).await?;
        read(&mut self.tx, job).await
    }
    pub async fn insert_media_derivation(&mut self, run: &MediaDerivationRun) -> Result<()> {
        run.request.validate()?;
        let (namespace, subject) = run.job.subject_scope()?;
        if *subject != run.request.subject_id
            || run.prepared.is_some()
            || run.description.is_some()
            || run.result.is_some()
        {
            return Err(Error::invalid(
                "accepted media work must have its exact subject and no pre-adopted output",
            ));
        }
        let observation = self
            .observation(namespace, &run.request.observation_id)
            .await?;
        let roots = self
            .media_roots(namespace, subject, &run.request.source)
            .await?;
        if observation.subject_id != *subject
            || observation.kind != run.source_event_kind
            || !roots.contains(&observation.material_id)
        {
            return Err(Error::invalid(
                "media derivation source event differs from its source and subject",
            ));
        }
        sqlx::query("INSERT INTO media_derivations(job_id,request,source_event_kind,description_target,result_target) VALUES(?,?,?,?,?)")
            .bind(&run.job.id).bind(serde_json::to_string(&run.request)?).bind(enum_name(&run.source_event_kind)?)
            .bind(json_option(&run.description_target)?).bind(serde_json::to_string(&run.result_target)?).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub async fn set_media_prepared(
        &mut self,
        id: &str,
        prepared: &MediaPreparedGraph,
    ) -> Result<()> {
        let run = self.media_derivation(id).await?;
        if let Some(current) = run.prepared {
            if serde_json::to_value(current)? != serde_json::to_value(prepared)? {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "prepared media directory is immutable",
                ));
            }
            return Ok(());
        }
        let (namespace, subject) = run.job.subject_scope()?;
        if prepared.inputs.is_empty() || prepared.inputs.len() > 64 {
            return Err(Error::invalid(
                "prepared media needs 1..64 committed inputs",
            ));
        }
        let admitted_roots = self
            .media_roots(namespace, subject, &run.request.source)
            .await?;
        let mut prepared_roots = std::collections::BTreeSet::new();
        for input in &prepared.inputs {
            prepared_roots.extend(self.media_roots(namespace, subject, &input.source).await?);
        }
        if prepared_roots != admitted_roots {
            return Err(Error::invalid(
                "prepared media directory differs from admitted original roots",
            ));
        }
        let duration = self
            .media_duration(namespace, subject, &run.request.source)
            .await?;
        prepared.validate(duration)?;
        sqlx::query("UPDATE media_derivations SET prepared=? WHERE job_id=? AND prepared IS NULL")
            .bind(serde_json::to_string(prepared)?)
            .bind(id)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn set_media_description(
        &mut self,
        id: &str,
        reference: &RepresentationRef,
    ) -> Result<()> {
        let run = self.media_derivation(id).await?;
        if run
            .description
            .as_ref()
            .is_some_and(|current| current != reference)
            || run.description_target.as_ref() != Some(reference)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "description differs from its reserved target",
            ));
        }
        let (namespace, subject) = run.job.subject_scope()?;
        self.representation(namespace, subject, reference).await?;
        sqlx::query("UPDATE media_derivations SET description=? WHERE job_id=?")
            .bind(serde_json::to_string(reference)?)
            .bind(id)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn set_media_result(
        &mut self,
        id: &str,
        reference: &RepresentationRef,
    ) -> Result<()> {
        let run = self.media_derivation(id).await?;
        if run
            .result
            .as_ref()
            .is_some_and(|current| current != reference)
            || run.result_target != *reference
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "result differs from its reserved target",
            ));
        }
        let (namespace, subject) = run.job.subject_scope()?;
        self.representation(namespace, subject, reference).await?;
        sqlx::query("UPDATE media_derivations SET result=? WHERE job_id=?")
            .bind(serde_json::to_string(reference)?)
            .bind(id)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
}
async fn read(conn: &mut SqliteConnection, job: JobRecord) -> Result<MediaDerivationRun> {
    let row = sqlx::query("SELECT * FROM media_derivations WHERE job_id=?")
        .bind(&job.id)
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    Ok(MediaDerivationRun {
        job,
        request: serde_json::from_str(row.get("request"))?,
        source_event_kind: parse_enum(row.get("source_event_kind"))?,
        description_target: parse_optional(row.get("description_target"))?,
        result_target: serde_json::from_str(row.get("result_target"))?,
        prepared: parse_optional(row.get("prepared"))?,
        description: parse_optional(row.get("description"))?,
        result: parse_optional(row.get("result"))?,
    })
}
