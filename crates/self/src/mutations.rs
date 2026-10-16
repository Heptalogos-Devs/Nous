// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl SelfService {
    pub async fn adopt(&self, mut input: AdoptSelf) -> Result<SelfRecord> {
        normalize(&mut input.content)?;
        input.content.validate()?;
        reason(&input.reason)?;
        let digest = canonical_request_digest("self.adopt", input.subject, &input)?;
        let mut mutation = match self
            .begin(input.subject, input.operation_id, "self.adopt", &digest)
            .await?
        {
            MutationStart::Replay(r) => {
                return self
                    .get(
                        input.subject,
                        r.result_revision.ok_or_else(|| {
                            Error::Infrastructure("Self receipt lacks revision".into())
                        })?,
                        true,
                    )
                    .await;
            }
            MutationStart::Active(m) => m,
        };
        self.validate_content(mutation.tx(), input.subject, &input.content)
            .await?;
        let object = Uuid::now_v7();
        let revision = Uuid::now_v7();
        let now = self.runtime.recorded_now();
        sqlx::query("INSERT INTO self_objects(object_id,subject_id,family,current_revision_id,created_at) VALUES($1,$2,$3,$4,$5)").bind(object).bind(input.subject.0).bind(input.content.family()).bind(revision).bind(now).execute(&mut **mutation.tx()).await.map_err(db)?;
        self.insert_revision(mutation.tx(), &input, object, revision, None, 1, now)
            .await?;
        let record = self
            .read_in(mutation.tx(), input.subject, revision, true)
            .await?;
        self.store
            .ensure_identity_addresses_in(
                mutation.tx(),
                input.subject,
                &[record.object_reference(), record.reference()],
                input.content.label(),
            )
            .await?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
        mutation
            .publish_workflow_results("self", &[record.object_reference(), record.reference()])
            .await?;
        mutation
            .commit(
                input.content.family(),
                Some(&object.to_string()),
                Some(revision),
                Some(1),
            )
            .await?;
        Ok(record)
    }
    pub async fn revise(&self, mut input: ReviseSelf) -> Result<SelfRecord> {
        normalize(&mut input.content)?;
        input.content.validate()?;
        reason(&input.reason)?;
        let digest = canonical_request_digest("self.revise", input.subject, &input)?;
        let mut mutation = match self
            .begin(input.subject, input.operation_id, "self.revise", &digest)
            .await?
        {
            MutationStart::Replay(r) => {
                return self
                    .get(
                        input.subject,
                        r.result_revision.ok_or_else(|| {
                            Error::Infrastructure("Self receipt lacks revision".into())
                        })?,
                        true,
                    )
                    .await;
            }
            MutationStart::Active(m) => m,
        };
        self.lock_object(
            mutation.tx(),
            input.subject,
            input.object_id,
            input.expected_epoch,
        )
        .await?;
        let current = self
            .read_in(mutation.tx(), input.subject, input.object_id, false)
            .await?;
        if current.current_revision_id != input.expected_revision
            || current.content.family() != input.content.family()
        {
            return Err(Error::Conflict("Self exact head or family changed".into()));
        }
        if current.acceptance_state != "accepted" || current.suppression_state != "normal" {
            return Err(Error::FailedPrecondition(
                "Self is not currently adopted and available".into(),
            ));
        }
        if let (SelfContent::SelfFacet(a), SelfContent::SelfFacet(b)) =
            (&current.content, &input.content)
            && a.kind != b.kind
        {
            return Err(Error::Invalid(
                "A different Self kind needs a distinct object".into(),
            ));
        }
        self.validate_content(mutation.tx(), input.subject, &input.content)
            .await?;
        if serde_json::to_value(&current.content).map_err(json_error)?
            == serde_json::to_value(&input.content).map_err(json_error)?
        {
            mutation
                .commit(
                    input.content.family(),
                    Some(&input.object_id.to_string()),
                    Some(current.revision_id),
                    Some(current.object_epoch),
                )
                .await?;
            return Ok(current);
        }
        let revision = Uuid::now_v7();
        let now = self.runtime.recorded_now();
        let adopt = AdoptSelf {
            subject: input.subject,
            operation_id: input.operation_id,
            content: input.content,
            reason: input.reason,
        };
        self.insert_revision(
            mutation.tx(),
            &adopt,
            input.object_id,
            revision,
            Some(current.revision_id),
            current.revision_no + 1,
            now,
        )
        .await?;
        AuthorityStore::invalidate_scoped_cognition_dependents_in(
            mutation.tx(),
            input.subject,
            current.reference().kind(),
            &[current.revision_id.to_string()],
        )
        .await?;
        sqlx::query("UPDATE self_objects SET current_revision_id=$2,integrity_state='valid',object_epoch=object_epoch+1 WHERE object_id=$1").bind(input.object_id).bind(revision).execute(&mut **mutation.tx()).await.map_err(db)?;
        let record = self
            .read_in(mutation.tx(), input.subject, revision, true)
            .await?;
        self.store
            .ensure_identity_addresses_in(
                mutation.tx(),
                input.subject,
                &[record.reference()],
                adopt.content.label(),
            )
            .await?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
        mutation
            .publish_workflow_results("self", &[record.object_reference(), record.reference()])
            .await?;
        mutation
            .commit(
                adopt.content.family(),
                Some(&input.object_id.to_string()),
                Some(revision),
                Some(record.object_epoch),
            )
            .await?;
        Ok(record)
    }
    pub async fn add_evidence(
        &self,
        subject: SubjectId,
        op: OperationId,
        target: CognitiveRef,
        epoch: i64,
        role: &str,
        basis: RevisionBasis,
    ) -> Result<SelfRecord> {
        if target.domain() != CognitiveDomain::SelfCognition || !target.is_cognition_revision() {
            return Err(Error::Invalid(
                "Self evidence needs an exact Self revision".into(),
            ));
        }
        let revision = target.uuid().expect("Self UUID");
        if !["support", "counterexample", "boundary_case"].contains(&role) {
            return Err(Error::Invalid("invalid Self evidence role".into()));
        }
        let digest =
            canonical_request_digest("self.evidence", subject, &(&target, epoch, role, &basis))?;
        let mut mutation = match self.begin(subject, op, "self.evidence", &digest).await? {
            MutationStart::Replay(_) => return self.get(subject, revision, true).await,
            MutationStart::Active(m) => m,
        };
        let record = self.read_in(mutation.tx(), subject, revision, true).await?;
        if record.reference() != target {
            return Err(Error::Invalid("Self reference family mismatch".into()));
        }
        self.lock_object(mutation.tx(), subject, record.object_id, epoch)
            .await?;
        self.validate_basis(mutation.tx(), subject, std::slice::from_ref(&basis))
            .await?;
        if let RevisionBasis::CognitionDependency(d) = &basis
            && AuthorityStore::cognition_dependency_reaches_in(
                mutation.tx(),
                subject,
                &d.target_revision,
                &target,
            )
            .await?
        {
            return Err(Error::Invalid("Self support would create a cycle".into()));
        }
        self.track_basis(mutation.tx(), revision, std::slice::from_ref(&basis))
            .await?;
        let count=sqlx::query("INSERT INTO self_evidence(revision_id,role,basis_key,basis,recorded_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING").bind(revision).bind(role).bind(basis.canonical_key()).bind(serde_json::to_value(basis).map_err(json_error)?).bind(self.runtime.recorded_now()).execute(&mut **mutation.tx()).await.map_err(db)?.rows_affected();
        if count > 0 {
            sqlx::query("UPDATE self_objects SET object_epoch=object_epoch+1 WHERE object_id=$1")
                .bind(record.object_id)
                .execute(&mut **mutation.tx())
                .await
                .map_err(db)?;
            mutation.invalidate(ProjectionInvalidation::all()).await?;
        }
        let result = self.read_in(mutation.tx(), subject, revision, true).await?;
        mutation
            .publish_workflow_results("self", &[result.object_reference(), result.reference()])
            .await?;
        mutation
            .commit(
                record.content.family(),
                Some(&record.object_id.to_string()),
                Some(revision),
                Some(result.object_epoch),
            )
            .await?;
        Ok(result)
    }
    async fn insert_revision(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        input: &AdoptSelf,
        object: Uuid,
        revision: Uuid,
        parent: Option<Uuid>,
        number: i32,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let producer = match &input.content {
            SelfContent::SelfFacet(v) => &v.producer,
            SelfContent::NarrativeThread(v) => &v.producer,
        };
        if let Some(p) = producer {
            if !matches!(
                p.operation,
                CapabilityOperation::SelfFormationText
                    | CapabilityOperation::SelfReflectionText
                    | CapabilityOperation::SubjectReflectionText
            ) {
                return Err(Error::Invalid(
                    "Self producer requires a Self formation/reflection operation".into(),
                ));
            }
            AuthorityStore::register_producer_in(tx, p).await?;
        }
        let formed = self
            .runtime
            .formation_stamp_in(tx, input.subject, "self", input.operation_id)
            .await?;
        sqlx::query("INSERT INTO self_revisions(revision_id,object_id,subject_id,revision_no,parent_revision_id,content,reason,formed_at,recorded_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(revision).bind(object).bind(input.subject.0).bind(number).bind(parent).bind(serde_json::to_value(&input.content).map_err(json_error)?).bind(&input.reason).bind(formed.at).bind(now).execute(&mut **tx).await.map_err(db)?;
        self.track_basis(tx, revision, input.content.basis())
            .await?;
        match &input.content {
            SelfContent::SelfFacet(v) => {
                for e in &v.exceptions {
                    self.track_basis(tx, revision, &e.basis).await?;
                }
            }
            SelfContent::NarrativeThread(v) => {
                for reference in &v.source_refs {
                    self.track_reference(tx, revision, reference).await?;
                }
            }
        }
        Ok(())
    }
    async fn track_basis(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        revision: Uuid,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        for b in basis {
            self.track_reference(tx, revision, &b.evidence_handle().reference)
                .await?;
            if let RevisionBasis::Evidence(e) = b {
                self.track_reference(tx, revision, &CognitiveRef::Occurrence(e.occurrence_id))
                    .await?;
            }
        }
        Ok(())
    }
    async fn track_reference(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        revision: Uuid,
        reference: &CognitiveRef,
    ) -> Result<()> {
        let (kind, value) = reference_parts(reference);
        sqlx::query("INSERT INTO self_dependencies(revision_id,ref_kind,ref_value,recorded_at) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(revision).bind(kind).bind(value).bind(Utc::now()).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }
    pub(crate) async fn lock_object(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        id: Uuid,
        epoch: i64,
    ) -> Result<()> {
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT object_epoch FROM self_objects WHERE subject_id=$1 AND object_id=$2 FOR UPDATE",
        )
        .bind(subject.0)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
        match current {
            Some(value) if value == epoch => Ok(()),
            Some(_) => Err(Error::Conflict("Self object epoch changed".into())),
            None => Err(Error::NotFound(
                "Self object is outside Subject or purged".into(),
            )),
        }
    }
}
fn reason(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 8192 {
        Err(Error::Invalid(
            "Self adoption/revision requires its actual reason".into(),
        ))
    } else {
        Ok(())
    }
}
fn json_error(error: serde_json::Error) -> Error {
    Error::Invalid(error.to_string())
}
fn normalize(content: &mut SelfContent) -> Result<()> {
    let producer = match content {
        SelfContent::SelfFacet(v) => &mut v.producer,
        SelfContent::NarrativeThread(v) => &mut v.producer,
    };
    *producer = producer
        .as_ref()
        .map(AuthorityStore::canonical_producer)
        .transpose()?;
    Ok(())
}
