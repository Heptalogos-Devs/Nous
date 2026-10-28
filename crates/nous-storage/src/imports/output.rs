use crate::{ReadView, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::*;

impl WriteTransaction {
    pub async fn insert_import_unit_output(&mut self, output: &ImportUnitOutput) -> Result<()> {
        self.check_sequence(output.commit_seq)?;
        output.id.validate()?;
        validate_name(&output.attempt_job_id, "unit processing attempt")?;
        if let Some(range) = &output.event_range {
            range.validate()?;
        }
        self.context()
            .require_subject(&output.namespace_id, &output.subject_id)?;
        let unit_body: Option<String> = sqlx::query_scalar(
            "SELECT body FROM import_text_units WHERE namespace_id=? AND subject_id=? AND id=?",
        )
        .bind(output.namespace_id.as_str())
        .bind(output.subject_id.as_str())
        .bind(output.unit_id.as_str())
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        let _: ImportTextUnit = serde_json::from_str(&unit_body.ok_or_else(not_found)?)?;
        for (id, temporal) in [
            (&output.semantic_call_id, false),
            (&output.temporal_call_id, true),
        ] {
            let Some(id) = id else {
                continue;
            };
            let call = self.model_call(id).await?;
            if call.namespace_id != output.namespace_id
                || call.subject_id != output.subject_id
                || (call.state != ModelCallState::Succeeded
                    && !(temporal
                        && output.temporal_error.is_some()
                        && matches!(
                            call.state,
                            ModelCallState::Failed | ModelCallState::Uncertain
                        )))
            {
                return Err(Error::invalid(
                    "import unit output differs from its actual scoped producer outcome",
                ));
            }
        }
        for id in &output.memory_ids {
            let known: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_revisions WHERE namespace_id=?1 AND subject_id=?2 AND memory_id=?3 UNION ALL SELECT 1 FROM memory_purges WHERE namespace_id=?1 AND subject_id=?2 AND memory_id=?3)")
                .bind(output.namespace_id.as_str()).bind(output.subject_id.as_str()).bind(id.as_str())
                .fetch_one(&mut *self.tx).await.map_err(storage_error)?;
            if !known {
                return Err(Error::invalid(
                    "unit output has no actual scoped Memory identity",
                ));
            }
        }
        for reference in &output.cognition {
            let object = crate::cognition::structured::read::object(
                &mut self.tx,
                &output.namespace_id,
                &CognitiveId(reference.id.clone()),
                Some(reference.revision),
            )
            .await?;
            if object.subject_id != output.subject_id || object.kind() != reference.kind {
                return Err(Error::invalid(
                    "unit output cognition differs from its scoped revision",
                ));
            }
        }
        sqlx::query("INSERT INTO import_unit_outputs(id,body) VALUES(?,?)")
            .bind(output.id.as_str())
            .bind(serde_json::to_string(output)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
}
impl ReadView {
    pub async fn import_unit_output(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        id: &ImportUnitOutputId,
    ) -> Result<ImportUnitOutput> {
        let body: Option<String> = sqlx::query_scalar("SELECT body FROM import_unit_outputs WHERE namespace_id=? AND subject_id=? AND id=? AND commit_seq<=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(id.as_str()).bind(self.watermark)
            .fetch_optional(&mut *self.tx).await.map_err(storage_error)?;
        Ok(serde_json::from_str(&body.ok_or_else(not_found)?)?)
    }
    pub async fn import_unit_output_facts(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<Vec<ImportUnitOutput>> {
        let bodies: Vec<String> = sqlx::query_scalar("SELECT body FROM import_unit_outputs WHERE namespace_id=? AND subject_id=? AND commit_seq<=? ORDER BY commit_seq,id")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(self.watermark)
            .fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        bodies
            .into_iter()
            .map(|body| serde_json::from_str(&body).map_err(Error::from))
            .collect()
    }
}
