use super::*;
use crate::{Database, WriteTransaction, write_error};
use serde_json::json;
use sha2::{Digest, Sha256};

async fn admission(
    conn: &mut SqliteConnection,
    id: &ContextAdmissionId,
) -> Result<ContextAdmission> {
    let body: String = sqlx::query_scalar("SELECT body FROM context_admissions WHERE id=?")
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    decode(&body)
}
impl Database {
    pub async fn context_admission(&self, id: &ContextAdmissionId) -> Result<ContextAdmission> {
        admission(&mut *self.pool.acquire().await.map_err(storage_error)?, id).await
    }
}
impl WriteTransaction {
    pub async fn context_admission(&mut self, id: &ContextAdmissionId) -> Result<ContextAdmission> {
        admission(&mut self.tx, id).await
    }
    pub async fn validate_context_targets(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        targets: &[RetrievalTarget],
    ) -> Result<()> {
        for target in targets {
            let context = self.context().clone();
            self.require_source_read(&context, ns, subject, target)
                .await?;
            let roots = match target {
                RetrievalTarget::Cognition(reference) => {
                    crate::vector::source::validate_reference(&mut self.tx, ns, subject, reference)
                        .await?;
                    crate::source::closure::closure(&mut self.tx, ns, subject, reference.clone())
                        .await?
                        .roots
                        .into_iter()
                        .map(|root| root.material_id)
                        .collect::<Vec<_>>()
                }
                RetrievalTarget::Media(reference) => self
                    .media_roots(ns, subject, reference)
                    .await?
                    .into_iter()
                    .collect(),
            };
            for material in roots {
                if !self
                    .subject_has_material_access(ns, subject, &material)
                    .await?
                {
                    return Err(Error::new(
                        ErrorCode::Forbidden,
                        "context source root is unavailable to this subject",
                    ));
                }
            }
        }
        Ok(())
    }
    pub async fn save_context_admission(&mut self, value: &ContextAdmission) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        self.context()
            .require_subject(&value.namespace_id, &value.subject_id)?;
        let session = session(&mut self.tx, &value.session_id).await?;
        if session.namespace_id != value.namespace_id
            || session.subject_id != value.subject_id
            || session.history_generation != value.history_generation
            || session.history_invalidated
            || session.closed_at.is_some()
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "native history must be refreshed before admission",
            ));
        }
        self.validate_context_targets(&value.namespace_id, &value.subject_id, &value.sources)
            .await?;
        sqlx::query("INSERT INTO context_admissions(id,projection_id,namespace_id,subject_id,session_id,consumer_id,instance_id,invocation_id,body,commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.projection_id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.session_id.as_str()).bind(&value.consumer_id).bind(&value.instance_id).bind(&value.invocation_id).bind(encode(value)?).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        for target in &value.sources {
            let event_id = format!("{}:{}", value.id, target.key()?);
            self.context_use_event(value, &event_id, target, "presented", &value.recorded_at)
                .await?;
        }
        Ok(())
    }
    async fn context_use_event(
        &mut self,
        admitted: &ContextAdmission,
        id: &str,
        target: &RetrievalTarget,
        kind: &str,
        occurred_at: &str,
    ) -> Result<bool> {
        let body = encode(
            &json!({"admission":admitted.id,"target":target,"kind":kind,"occurred_at":occurred_at}),
        )?;
        let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
        let existing:Option<String>=sqlx::query_scalar("SELECT request_hash FROM context_use_events WHERE namespace_id=? AND subject_id=? AND consumer_id=? AND instance_id=? AND event_id=?")
            .bind(admitted.namespace_id.as_str()).bind(admitted.subject_id.as_str()).bind(&admitted.consumer_id).bind(&admitted.instance_id).bind(id).fetch_optional(&mut *self.tx).await.map_err(storage_error)?;
        if let Some(existing) = existing {
            if existing != hash {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "context use event id has different content",
                ));
            }
            return Ok(false);
        }
        sqlx::query("INSERT INTO context_use_events(namespace_id,subject_id,consumer_id,instance_id,event_id,admission_id,target,kind,occurred_at,recorded_at,request_hash,commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(admitted.namespace_id.as_str()).bind(admitted.subject_id.as_str()).bind(&admitted.consumer_id).bind(&admitted.instance_id).bind(id).bind(admitted.id.as_str()).bind(encode(target)?).bind(kind).bind(occurred_at).bind(self.time().recorded()).bind(hash).bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(true)
    }
    pub async fn record_context_report(
        &mut self,
        request: &ContextReportRequest,
    ) -> Result<(ContextAdmission, usize)> {
        request.validate()?;
        let admitted = admission(&mut self.tx, &request.admission_id).await?;
        self.context()
            .require_subject(&admitted.namespace_id, &admitted.subject_id)?;
        if admitted.invocation_id != request.invocation_id {
            return Err(Error::invalid(
                "outcome does not match its admitted invocation",
            ));
        }
        let outcome = encode(
            &json!({"admission_id":request.admission_id,"invocation_id":request.invocation_id,"outcome":request.outcome,"occurred_at":request.occurred_at}),
        )?;
        let previous: Option<String> =
            sqlx::query_scalar("SELECT body FROM context_outcomes WHERE admission_id=?")
                .bind(admitted.id.as_str())
                .fetch_optional(&mut *self.tx)
                .await
                .map_err(storage_error)?;
        if previous
            .as_ref()
            .is_some_and(|previous| previous != &outcome)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "invocation outcome is already recorded differently",
            ));
        }
        if previous.is_none() {
            sqlx::query("INSERT INTO context_outcomes(admission_id,body,recorded_at,commit_seq) VALUES(?,?,?,?)").bind(admitted.id.as_str()).bind(outcome).bind(self.time().recorded()).bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        let mut added = 0;
        for event in &request.uses {
            if !admitted.sources.contains(&event.target) {
                return Err(Error::invalid(
                    "context use must cite an actually admitted source",
                ));
            }
            let kind = serde_json::to_value(event.kind)?
                .as_str()
                .expect("use kind string")
                .to_owned();
            added += usize::from(
                self.context_use_event(
                    &admitted,
                    &event.event_id,
                    &event.target,
                    &kind,
                    &request.occurred_at,
                )
                .await?,
            );
        }
        Ok((admitted, added))
    }
    pub async fn invalidate_context_history(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        targets: &[RetrievalTarget],
    ) -> Result<u64> {
        let mut changed = 0;
        for target in targets {
            changed+=sqlx::query("WITH RECURSIVE seeds(id) AS (SELECT a.session_id FROM context_admissions a WHERE a.namespace_id=? AND a.subject_id=? AND (EXISTS(SELECT 1 FROM json_each(a.body,'$.sources') r WHERE r.value=?) OR EXISTS(SELECT 1 FROM json_each(a.body,'$.dependencies') r WHERE r.value=?))), parents(id) AS (SELECT id FROM seeds UNION SELECT s.parent_id FROM context_sessions s JOIN parents p ON s.id=p.id WHERE s.parent_id IS NOT NULL), affected(id) AS (SELECT id FROM parents UNION SELECT s.id FROM context_sessions s JOIN affected p ON s.parent_id=p.id) UPDATE context_sessions SET history_invalidated=1,revision=?,commit_seq=?,body=json_set(body,'$.history_invalidated',json('true'),'$.revision',?,'$.commit_seq',?,'$.recorded_at',?) WHERE namespace_id=? AND subject_id=? AND id IN(SELECT id FROM affected) AND history_invalidated=0")
                .bind(ns.as_str()).bind(subject.as_str()).bind(encode(target)?).bind(encode(target)?).bind(self.commit_seq()).bind(self.commit_seq()).bind(self.commit_seq()).bind(self.commit_seq()).bind(self.time().recorded()).bind(ns.as_str()).bind(subject.as_str()).execute(&mut *self.tx).await.map_err(write_error)?.rows_affected();
        }
        Ok(changed)
    }
}
