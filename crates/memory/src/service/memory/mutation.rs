// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn form_memory(&self, mut input: ExplicitMemoryInput) -> Result<MemoryView> {
        let started_at = self.cognition.now(input.subject);
        input.producer = input
            .producer
            .as_ref()
            .map(AuthorityStore::canonical_producer)
            .transpose()?;
        input.validate()?;
        input.tags.sort_by_key(|tag| tag.0);
        input.tags.dedup();
        self.store.require_subject(input.subject).await?;
        let digest = operation_digest(
            "form_memory",
            input.subject,
            &serde_json::json!({"cognitive_role":input.cognitive_role,"formation_mode":input.formation_mode,"grounding_occurrence_id":input.grounding_occurrence_id,"semantic_role":input.semantic_role,"representation_text":input.representation_text,"title":input.title,"basis":input.basis,"aboutness":input.aboutness,"tags":input.tags,"valid_time":input.valid_time,"epistemic_class":input.epistemic_class,"producer":input.producer}),
        )?;
        let mut mutation = match self
            .start_mutation(input.subject, input.operation_id, "form_memory", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return self
                        .memory(
                            input.subject,
                            MemoryId(
                                receipt
                                    .result_ref
                                    .ok_or_else(|| {
                                        Error::Infrastructure("form receipt has no result".into())
                                    })?
                                    .parse()
                                    .map_err(|_| {
                                        Error::Infrastructure("invalid form receipt".into())
                                    })?,
                            ),
                            receipt.result_revision.map(MemoryRevisionId),
                        )
                        .await;
                }
                return Err(Error::Unavailable(
                    "form_memory operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        self.validate_basis_for_subject(input.subject, &input.basis)
            .await?;
        self.validate_formation_semantics(input.subject, input.formation_mode, &input.basis)
            .await?;
        let formed_at = self
            .formation_time_in(mutation.tx(), input.subject, input.operation_id, started_at)
            .await?;
        let (memory_id, revision_id) = self
            .create_memory_in(mutation.tx(), &input, formed_at)
            .await?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.enqueue_concept_in(
            mutation.tx(),
            input.subject,
            CognitiveRef::MemoryRevision(revision_id),
            sequence,
        )
        .await?;

        mutation
            .publish_workflow_results(
                "memory",
                &[
                    CognitiveRef::Memory(memory_id),
                    CognitiveRef::MemoryRevision(revision_id),
                ],
            )
            .await?;
        mutation
            .commit(
                "memory",
                Some(&memory_id.0.to_string()),
                Some(revision_id.0),
                Some(1),
            )
            .await?;
        self.memory(input.subject, memory_id, Some(revision_id))
            .await
    }

    pub(in crate::service) async fn create_memory_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &ExplicitMemoryInput,
        formed_at: DateTime<Utc>,
    ) -> Result<(MemoryId, MemoryRevisionId)> {
        let memory_id = MemoryId::new();
        let revision_id = MemoryRevisionId::new();
        let now = self.cognition.now(input.subject);
        self.validate_basis_in_tx(tx, input.subject, &input.basis)
            .await?;
        sqlx::query("INSERT INTO memory_objects(memory_id,subject_id,cognitive_role,current_revision_id,object_epoch,acceptance_state,integrity_state,suppression_state,purge_state,accessibility_mode,created_at) VALUES($1,$2,$3,$4,1,'accepted','valid','normal','normal','auto',$5)")
            .bind(memory_id.0).bind(input.subject.0).bind(input.cognitive_role.as_str()).bind(revision_id.0).bind(now).execute(&mut **tx).await.map_err(db)?;
        self.insert_revision_in_tx(
            tx,
            input,
            memory_id,
            revision_id,
            None,
            None,
            1,
            formed_at,
            now,
        )
        .await?;
        Ok((memory_id, revision_id))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "revision insertion keeps immutable content and support commit together"
    )]
    pub(in crate::service) async fn insert_revision_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &ExplicitMemoryInput,
        memory_id: MemoryId,
        revision_id: MemoryRevisionId,
        parent: Option<MemoryRevisionId>,
        revision_intent: Option<RevisionIntent>,
        revision_no: i32,
        formed_at: DateTime<Utc>,
        recorded_at: DateTime<Utc>,
    ) -> Result<()> {
        let producer_id = if let Some(p) = &input.producer {
            if !matches!(
                p.operation,
                CapabilityOperation::MemoryFormationText
                    | CapabilityOperation::MemoryConsolidationText
            ) {
                return Err(Error::Invalid(
                    "Memory producer requires a Memory formation operation".into(),
                ));
            }
            Some(AuthorityStore::register_producer_in(tx, p).await?)
        } else {
            None
        };
        let (valid_kind, valid_start, valid_end) = temporal_columns(&input.valid_time);
        sqlx::query("INSERT INTO memory_revisions(memory_revision_id,memory_id,subject_id,revision_no,parent_revision_id,revision_intent,formation_mode,grounding_occurrence_id,semantic_role,title,representation_text,epistemic_class,valid_time_kind,valid_time_start,valid_time_end,formed_at,recorded_at,producer_signature_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)")
            .bind(revision_id.0).bind(memory_id.0).bind(input.subject.0).bind(revision_no).bind(parent.map(|id| id.0)).bind(revision_intent.map(|value| value.as_str())).bind(input.formation_mode.as_str()).bind(input.grounding_occurrence_id.map(|id| id.0)).bind(&input.semantic_role).bind(&input.title).bind(&input.representation_text).bind(format!("{:?}",input.epistemic_class).to_lowercase()).bind(valid_kind).bind(valid_start).bind(valid_end).bind(formed_at).bind(recorded_at).bind(producer_id).execute(&mut **tx).await.map_err(db)?;
        for (index, basis) in input.basis.iter().enumerate() {
            self.insert_basis_in_tx(tx, revision_id, index as i32, basis)
                .await?;
        }
        for entity in &input.aboutness {
            sqlx::query("INSERT INTO memory_revision_aboutness(memory_revision_id,entity_ref) VALUES($1,$2)").bind(revision_id.0).bind(entity.as_str()).execute(&mut **tx).await.map_err(db)?;
        }
        let mut canonical_tags = std::collections::BTreeSet::new();
        for tag in &input.tags {
            let canonical: Option<Uuid> = sqlx::query_scalar("SELECT canonical_tag($1,$2)")
                .bind(input.subject.0)
                .bind(tag.0)
                .fetch_one(&mut **tx)
                .await
                .map_err(db)?;
            let canonical = canonical
                .ok_or_else(|| Error::Invalid("active Tag does not belong to Subject".into()))?;
            canonical_tags.insert(canonical);
        }
        for tag in canonical_tags {
            sqlx::query(
                "INSERT INTO memory_revision_tags(memory_revision_id,tag_id) VALUES($1,$2)",
            )
            .bind(revision_id.0)
            .bind(tag)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        }
        self.store
            .ensure_identity_addresses_in(
                tx,
                input.subject,
                &[
                    CognitiveRef::Memory(memory_id),
                    CognitiveRef::MemoryRevision(revision_id),
                ],
                input.title.as_deref().unwrap_or(""),
            )
            .await?;
        Ok(())
    }

    pub(in crate::service) async fn insert_basis_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        revision: MemoryRevisionId,
        evidence_no: i32,
        basis: &RevisionBasis,
    ) -> Result<()> {
        match basis {
            RevisionBasis::Evidence(evidence) => {
                let (source_region, derived_representation, derived_region) = match evidence.locator
                {
                    EvidenceLocator::WholeOccurrence => (None, None, None),
                    EvidenceLocator::SourceRegion(id) => (Some(id.0), None, None),
                    EvidenceLocator::DerivedRepresentation(id) => (None, Some(id.0), None),
                    EvidenceLocator::DerivedRegion(id) => (None, None, Some(id.0)),
                };
                sqlx::query("INSERT INTO memory_revision_evidence(memory_revision_id,occurrence_id,evidence_no,source_region_id,derived_representation_id,derived_region_id,basis_role,epistemic_relation) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
                    .bind(revision.0).bind(evidence.occurrence_id.0).bind(evidence_no).bind(source_region).bind(derived_representation).bind(derived_region).bind(evidence.basis_role.as_str()).bind(epistemic_relation_text(evidence.epistemic_relation)).execute(&mut **tx).await.map_err(db)?;
            }
            RevisionBasis::CognitionDependency(dependency) => {
                let (kind, value) = reference_parts(&dependency.target_revision);
                if !matches!(
                    dependency.target_revision,
                    CognitiveRef::MemoryRevision(_)
                        | CognitiveRef::CognitiveSchemaRevision(_)
                        | CognitiveRef::EpisodeRevision(_)
                        | CognitiveRef::JournalRevision(_)
                ) {
                    return Err(Error::Invalid(
                        "cognition dependency must target an exact revision".into(),
                    ));
                }
                sqlx::query("INSERT INTO memory_revision_dependencies(memory_revision_id,target_ref_kind,target_ref,basis_role,epistemic_relation) VALUES($1,$2,$3,$4,$5)").bind(revision.0).bind(kind).bind(value).bind(dependency.basis_role.as_str()).bind(epistemic_relation_text(dependency.epistemic_relation)).execute(&mut **tx).await.map_err(db)?;
            }
            RevisionBasis::Seed(_) => {
                return Err(Error::Invalid(
                    "Memory revisions cannot use Cognitive Seed support".into(),
                ));
            }
        }
        Ok(())
    }

    pub async fn commit_formation_proposal(
        &self,
        subject: SubjectId,
        proposal: MemoryFormationProposal,
        allowed_entity_refs: &[EntityRef],
    ) -> Result<MemoryView> {
        if proposal
            .aboutness
            .iter()
            .any(|entity| !allowed_entity_refs.contains(entity))
        {
            return Err(Error::Invalid(
                "formation proposal contains an EntityRef outside Host context".into(),
            ));
        }
        self.form_memory(proposal.into_input(subject)).await
    }

    pub async fn revise_memory(&self, mut input: ReviseMemoryInput) -> Result<MemoryView> {
        let started_at = self.cognition.now(input.subject);
        input.producer = input
            .producer
            .as_ref()
            .map(AuthorityStore::canonical_producer)
            .transpose()?;
        if input.operation_id.0.is_nil() {
            return Err(Error::Invalid("operation_id is required".into()));
        }
        validate_content(&input.semantic_role, &input.representation_text)?;
        input.valid_time.validate()?;
        self.store.require_subject(input.subject).await?;
        self.validate_basis_for_subject(input.subject, &input.basis)
            .await?;
        self.validate_formation_semantics(input.subject, input.formation_mode, &input.basis)
            .await?;
        let digest = operation_digest(
            "revise_memory",
            input.subject,
            &serde_json::json!({"memory_id":input.memory_id,"expected_object_epoch":input.expected_object_epoch,"intent":input.intent,"formation_mode":input.formation_mode,"grounding_occurrence_id":input.grounding_occurrence_id,"semantic_role":input.semantic_role,"representation_text":input.representation_text,"title":input.title,"basis":input.basis,"aboutness":input.aboutness,"valid_time":input.valid_time,"epistemic_class":input.epistemic_class,"producer":input.producer}),
        )?;
        let mut mutation = match self
            .start_mutation(input.subject, input.operation_id, "revise_memory", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return self
                        .memory(
                            input.subject,
                            input.memory_id,
                            receipt.result_revision.map(MemoryRevisionId),
                        )
                        .await;
                }
                return Err(Error::Unavailable(
                    "revise_memory operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let formed_at = self
            .formation_time_in(mutation.tx(), input.subject, input.operation_id, started_at)
            .await?;
        let (revision_id, epoch) = self
            .revise_memory_in(mutation.tx(), &input, formed_at)
            .await?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.enqueue_concept_in(
            mutation.tx(),
            input.subject,
            CognitiveRef::MemoryRevision(revision_id),
            sequence,
        )
        .await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            input.subject,
            "memory",
            &[input.memory_id.0],
            sequence,
            "source_revised",
        )
        .await?;

        mutation
            .publish_workflow_results(
                "memory",
                &[
                    CognitiveRef::Memory(input.memory_id),
                    CognitiveRef::MemoryRevision(revision_id),
                ],
            )
            .await?;
        mutation
            .commit(
                "memory_revision",
                Some(&input.memory_id.0.to_string()),
                Some(revision_id.0),
                Some(epoch + 1),
            )
            .await?;
        self.memory(input.subject, input.memory_id, Some(revision_id))
            .await
    }

    pub(in crate::service) async fn revise_memory_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &ReviseMemoryInput,
        formed_at: DateTime<Utc>,
    ) -> Result<(MemoryRevisionId, i64)> {
        let row = sqlx::query("SELECT current_revision_id,object_epoch,cognitive_role,purge_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE")
            .bind(input.subject.0).bind(input.memory_id.0).fetch_optional(&mut **tx).await.map_err(db)?.ok_or_else(|| Error::NotFound("memory not found".into()))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        fence_epoch(epoch, input.expected_object_epoch)?;
        if row.try_get::<String, _>("purge_state").map_err(db)? == "purging" {
            return Err(Error::FailedPrecondition("memory is purging".into()));
        }
        self.validate_object_dependency_cycle(
            input.subject,
            &format!("memory:{}", input.memory_id.0),
            &input.basis.clone(),
        )
        .await?;
        self.validate_basis_in_tx(tx, input.subject, &input.basis)
            .await?;
        let parent = MemoryRevisionId(row.try_get("current_revision_id").map_err(db)?);
        let parent_aboutness = sqlx::query_scalar::<_, String>(
            "SELECT entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=$1",
        )
        .bind(parent.0)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
        if !parent_aboutness.is_empty()
            && !input.aboutness.iter().any(|entity| {
                parent_aboutness
                    .iter()
                    .any(|value| value == entity.as_str())
            })
        {
            return Err(Error::FailedPrecondition(
                "revision aboutness is disjoint from the existing cognitive matter; form a new Memory object".into(),
            ));
        }
        let revision_no: i32 = sqlx::query_scalar(
            "SELECT revision_no+1 FROM memory_revisions WHERE memory_revision_id=$1",
        )
        .bind(parent.0)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
        let revision_id = MemoryRevisionId::new();
        let role: CognitiveRole =
            parse_enum(row.try_get("cognitive_role").map_err(db)?, "cognitive role")?;
        let input_for_insert = ExplicitMemoryInput {
            producer: input.producer.clone(),
            operation_id: input.operation_id,
            subject: input.subject,
            cognitive_role: role,
            formation_mode: input.formation_mode,
            grounding_occurrence_id: input.grounding_occurrence_id,
            semantic_role: input.semantic_role.clone(),
            representation_text: input.representation_text.clone(),
            title: input.title.clone(),
            basis: input.basis.clone(),
            aboutness: input.aboutness.clone(),
            tags: Vec::new(),
            valid_time: input.valid_time.clone(),

            epistemic_class: input.epistemic_class,
        };
        input_for_insert.validate()?;
        self.insert_revision_in_tx(
            tx,
            &input_for_insert,
            input.memory_id,
            revision_id,
            Some(parent),
            Some(input.intent),
            revision_no,
            formed_at,
            self.cognition.now(input.subject),
        )
        .await?;
        sqlx::query("UPDATE memory_objects SET current_revision_id=$3,object_epoch=object_epoch+1,integrity_state='valid' WHERE subject_id=$1 AND memory_id=$2").bind(input.subject.0).bind(input.memory_id.0).bind(revision_id.0).execute(&mut **tx).await.map_err(db)?;
        Ok((revision_id, epoch))
    }

    pub async fn link_revisions(
        &self,
        subject: SubjectId,
        operation_id: OperationId,
        from: MemoryRevisionId,
        to: MemoryRevisionId,
        relation: MemoryRelation,
    ) -> Result<()> {
        if from == to {
            return Err(Error::Invalid("a relation cannot target itself".into()));
        }
        self.store
            .validate_reference(subject, &CognitiveRef::MemoryRevision(from))
            .await?;
        self.store
            .validate_reference(subject, &CognitiveRef::MemoryRevision(to))
            .await?;
        let digest = operation_digest(
            "link_memory_revisions",
            subject,
            &serde_json::json!({"from":from,"to":to,"relation":relation}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation_id, "link_memory_revisions", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return Ok(());
                }
                return Err(Error::Unavailable(
                    "relation operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        sqlx::query("INSERT INTO memory_revision_relations(from_revision_id,to_revision_id,relation,created_at) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(from.0).bind(to.0).bind(relation.as_str()).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        mutation
            .invalidate(ProjectionInvalidation::topology())
            .await?;

        mutation.commit("relation", None, None, None).await
    }
}
