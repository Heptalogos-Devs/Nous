// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl MotivationService {
    pub async fn lifecycle(
        &self,
        subject: SubjectId,
        op: OperationId,
        target: CognitiveRef,
        epoch: i64,
        action: &str,
    ) -> Result<MotivationRecord> {
        let object = object_id(&target)?;
        let (field, from, to) = match action {
            "suppress" => ("suppression_state", "normal", "suppressed"),
            "restore" => ("suppression_state", "suppressed", "normal"),
            "withdraw" => ("acceptance_state", "accepted", "withdrawn"),
            "reaccept" => ("acceptance_state", "withdrawn", "accepted"),
            _ => return Err(Error::Invalid("unknown Motivation lifecycle action".into())),
        };
        let digest =
            canonical_request_digest("motivation.lifecycle", subject, &(&target, epoch, action))?;
        let mut mutation = match self
            .begin(subject, op, "motivation.lifecycle", &digest)
            .await?
        {
            MutationStart::Replay(r) => {
                return self
                    .get(
                        subject,
                        r.result_revision.ok_or_else(|| {
                            Error::Infrastructure(
                                "Motivation lifecycle receipt lacks revision".into(),
                            )
                        })?,
                        true,
                    )
                    .await;
            }
            MutationStart::Active(m) => m,
        };
        self.lock_object(mutation.tx(), subject, object, epoch)
            .await?;
        let before = self.read_in(mutation.tx(), subject, object, false).await?;
        if before.object_reference() != target {
            return Err(Error::Invalid(
                "Motivation reference family mismatch".into(),
            ));
        }
        let current = if field == "suppression_state" {
            &before.suppression_state
        } else {
            &before.acceptance_state
        };
        if current != from {
            return Err(Error::FailedPrecondition(
                "Motivation lifecycle transition is not applicable".into(),
            ));
        }
        sqlx::query("UPDATE motivation_objects SET suppression_state=CASE WHEN $2='suppression_state' THEN $3 ELSE suppression_state END,acceptance_state=CASE WHEN $2='acceptance_state' THEN $3 ELSE acceptance_state END,object_epoch=object_epoch+1 WHERE object_id=$1").bind(object).bind(field).bind(to).execute(&mut **mutation.tx()).await.map_err(db)?;
        let revisions = self.revision_ids(mutation.tx(), object).await?;
        AuthorityStore::invalidate_scoped_cognition_dependents_in(
            mutation.tx(),
            subject,
            before.reference().kind(),
            &revisions,
        )
        .await?;
        if matches!(action, "suppress" | "withdraw") {
            self.runtime
                .invalidate_session_history_in(
                    mutation.tx(),
                    subject,
                    before.reference().kind(),
                    &revisions,
                )
                .await?;
        }
        self.runtime
            .forget_resident_cognition_in(
                mutation.tx(),
                subject,
                before.reference().kind(),
                &revisions,
            )
            .await?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
        let result = self.read_in(mutation.tx(), subject, object, false).await?;
        mutation
            .commit(
                before.content.family(),
                Some(&object.to_string()),
                Some(before.revision_id),
                Some(result.object_epoch),
            )
            .await?;
        Ok(result)
    }
    pub async fn purge(
        &self,
        subject: SubjectId,
        op: OperationId,
        target: CognitiveRef,
        epoch: i64,
    ) -> Result<()> {
        let object = object_id(&target)?;
        let digest = canonical_request_digest("motivation.purge", subject, &(&target, epoch))?;
        let mut mutation = match self.begin(subject, op, "motivation.purge", &digest).await? {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(m) => m,
        };
        self.lock_object(mutation.tx(), subject, object, epoch)
            .await?;
        let record = self.read_in(mutation.tx(), subject, object, false).await?;
        if record.object_reference() != target {
            return Err(Error::Invalid(
                "Motivation reference family mismatch".into(),
            ));
        }
        let revisions = self.revision_ids(mutation.tx(), object).await?;
        AuthorityStore::invalidate_scoped_cognition_dependents_in(
            mutation.tx(),
            subject,
            record.reference().kind(),
            &revisions,
        )
        .await?;
        let mut refs = revisions.clone();
        refs.push(object.to_string());
        AuthorityStore::purge_model_results_in(mutation.tx(), subject, &refs).await?;
        self.runtime
            .purge_cognition_refs_in(
                mutation.tx(),
                subject,
                record.reference().kind(),
                &revisions,
            )
            .await?;
        sqlx::query("UPDATE lexical_bindings SET tombstoned_at=COALESCE(tombstoned_at,$3) WHERE object_kind=ANY($1::text[]) AND canonical_ref=ANY($2::text[])").bind(vec![record.content.family().to_string(),record.reference().kind().to_string()]).bind(&refs).bind(self.runtime.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("UPDATE lexical_visibility v SET display_name='',aliases='{}' FROM lexical_bindings b WHERE v.lexical_ref=b.lexical_ref AND v.subject_id=$1 AND b.object_kind=ANY($2::text[]) AND b.canonical_ref=ANY($3::text[])").bind(subject.0).bind(vec![record.content.family().to_string(),record.reference().kind().to_string()]).bind(&refs).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM motivation_objects WHERE subject_id=$1 AND object_id=$2")
            .bind(subject.0)
            .bind(object)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
        mutation
            .commit("purged", Some(&object.to_string()), None, None)
            .await
    }
    async fn revision_ids(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        object: Uuid,
    ) -> Result<Vec<String>> {
        sqlx::query_scalar(
            "SELECT revision_id::text FROM motivation_revisions WHERE object_id=$1 ORDER BY revision_id",
        )
        .bind(object)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)
    }
}

fn object_id(reference: &CognitiveRef) -> Result<Uuid> {
    if reference.domain() != CognitiveDomain::Motivation || reference.is_cognition_revision() {
        return Err(Error::Invalid(
            "Motivation lifecycle requires an object reference".into(),
        ));
    }
    Ok(reference.uuid().expect("Motivation UUID"))
}
