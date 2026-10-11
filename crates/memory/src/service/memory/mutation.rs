// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn form_memory(&self, mut input: ExplicitMemoryInput) -> Result<MutationResult> {
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
            .start_mutation(
                ErrorOwner::Memory,
                input.subject,
                input.operation_id,
                "form_memory",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                return single_mutation_result(receipt, ErrorOwner::Memory);
            }
            MutationStart::Active(mutation) => mutation,
        };
        self.validate_basis_for_subject(ErrorOwner::Memory, input.subject, &input.basis)
            .await?;
        self.validate_formation_semantics(
            ErrorOwner::Memory,
            input.subject,
            input.formation_mode,
            &input.basis,
        )
        .await?;
        let formed_at = started_at;
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
        let result =
            MutationResult::revised(CognitiveRef::Memory(memory_id), revision_id.0, Some(1));
        mutation.commit(std::slice::from_ref(&result)).await?;
        Ok(result)
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
        self.validate_basis_in_tx(ErrorOwner::Memory, tx, input.subject, &input.basis)
            .await?;
        sqlx::query("INSERT INTO cognition_objects(object_kind,object_id,subject_id,current_revision_id,object_epoch,acceptance_state,integrity_state,suppression_state,purge_state,created_at,cognitive_role,accessibility_mode) VALUES('memory',$1,$2,$3,1,'accepted','valid','normal','normal',$4,$5,'auto')")
            .bind(memory_id.0).bind(input.subject.0).bind(revision_id.0).bind(now).bind(input.cognitive_role.as_str()).execute(&mut **tx).await.map_err(db)?;
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
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "Memory producer requires a Memory formation operation",
                ));
            }
            Some(AuthorityStore::register_producer_in(tx, p).await?)
        } else {
            None
        };
        let (valid_kind, valid_start, valid_end) = temporal_columns(&input.valid_time);
        Self::insert_cognition_revision_header_in(
            &self.store,
            tx,
            input.subject,
            &CognitionRevisionHeader::new(
                CognitionKind::Memory,
                memory_id.0,
                revision_id.0,
                revision_no,
                parent.map(|id| id.0),
                formed_at,
                recorded_at,
                producer_id,
            ),
            input.title.as_deref().unwrap_or(""),
        )
        .await?;
        sqlx::query("INSERT INTO memory_revisions(memory_revision_id,revision_intent,formation_mode,grounding_occurrence_id,semantic_role,title,representation_text,epistemic_class,valid_time_kind,valid_time_start,valid_time_end) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(revision_id.0).bind(revision_intent.map(|value| value.as_str())).bind(input.formation_mode.as_str()).bind(input.grounding_occurrence_id.map(|id| id.0)).bind(&input.semantic_role).bind(&input.title).bind(&input.representation_text).bind(input.epistemic_class.as_str()).bind(valid_kind).bind(valid_start).bind(valid_end).execute(&mut **tx).await.map_err(db)?;
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
            let canonical = canonical.ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "active Tag does not belong to Subject",
                )
            })?;
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
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "cognition dependency must target an exact revision",
                    ));
                }
                sqlx::query("INSERT INTO memory_revision_dependencies(memory_revision_id,target_ref_kind,target_ref,basis_role,epistemic_relation) VALUES($1,$2,$3,$4,$5)").bind(revision.0).bind(kind).bind(value).bind(dependency.basis_role.as_str()).bind(epistemic_relation_text(dependency.epistemic_relation)).execute(&mut **tx).await.map_err(db)?;
            }
            RevisionBasis::Seed(_) => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "Memory revisions cannot use Cognitive Seed support",
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
    ) -> Result<MutationResult> {
        if proposal
            .aboutness
            .iter()
            .any(|entity| !allowed_entity_refs.contains(entity))
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "formation proposal contains an EntityRef outside Host context",
            ));
        }
        self.form_memory(proposal.into_input(subject)).await
    }

    pub async fn revise_memory(&self, mut input: ReviseMemoryInput) -> Result<MutationResult> {
        let started_at = self.cognition.now(input.subject);
        input.producer = input
            .producer
            .as_ref()
            .map(AuthorityStore::canonical_producer)
            .transpose()?;
        input.tags.sort_by_key(|tag| tag.0);
        input.tags.dedup();
        if input.operation_id.0.is_nil() {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "operation_id is required",
            ));
        }
        validate_content(&input.semantic_role, &input.representation_text)?;
        input.valid_time.validate()?;
        self.store.require_subject(input.subject).await?;
        self.validate_basis_for_subject(ErrorOwner::Memory, input.subject, &input.basis)
            .await?;
        self.validate_formation_semantics(
            ErrorOwner::Memory,
            input.subject,
            input.formation_mode,
            &input.basis,
        )
        .await?;
        let digest = operation_digest(
            "revise_memory",
            input.subject,
            &serde_json::json!({"memory_id":input.memory_id,"expected_object_epoch":input.expected_object_epoch,"intent":input.intent,"formation_mode":input.formation_mode,"grounding_occurrence_id":input.grounding_occurrence_id,"semantic_role":input.semantic_role,"representation_text":input.representation_text,"title":input.title,"basis":input.basis,"aboutness":input.aboutness,"tags":input.tags,"valid_time":input.valid_time,"epistemic_class":input.epistemic_class,"producer":input.producer}),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Memory,
                input.subject,
                input.operation_id,
                "revise_memory",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                return single_mutation_result(receipt, ErrorOwner::Memory);
            }
            MutationStart::Active(mutation) => mutation,
        };
        let formed_at = started_at;
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
        let result = MutationResult::revised(
            CognitiveRef::Memory(input.memory_id),
            revision_id.0,
            Some(epoch + 1),
        );
        mutation.commit(std::slice::from_ref(&result)).await?;
        Ok(result)
    }

    pub(in crate::service) async fn revise_memory_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &ReviseMemoryInput,
        formed_at: DateTime<Utc>,
    ) -> Result<(MemoryRevisionId, i64)> {
        let row = sqlx::query("SELECT current_revision_id,object_epoch,cognitive_role,purge_state FROM cognition_objects WHERE object_kind='memory' AND subject_id=$1 AND object_id=$2 FOR UPDATE")
            .bind(input.subject.0).bind(input.memory_id.0).fetch_optional(&mut **tx).await.map_err(db)?.ok_or_else(|| Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::NotFound, "memory not found"))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        fence_epoch(ErrorOwner::Memory, epoch, input.expected_object_epoch)?;
        if row.try_get::<String, _>("purge_state").map_err(db)? == "purging" {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::FailedPrecondition,
                "memory is purging",
            ));
        }
        self.validate_object_dependency_cycle(
            ErrorOwner::Memory,
            input.subject,
            &format!("memory:{}", input.memory_id.0),
            &input.basis.clone(),
        )
        .await?;
        self.validate_basis_in_tx(ErrorOwner::Memory, tx, input.subject, &input.basis)
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
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::FailedPrecondition,
                "revision aboutness is disjoint from the existing cognitive matter; form a new Memory object",
            ));
        }
        let revision_no: i32 = sqlx::query_scalar(
            "SELECT revision_no+1 FROM cognition_revision_headers WHERE object_kind='memory' AND revision_id=$1",
        )
        .bind(parent.0)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
        let revision_id = MemoryRevisionId::new();
        let role: CognitiveRole = parse_enum(
            ErrorOwner::Memory,
            row.try_get("cognitive_role").map_err(db)?,
            "cognitive role",
        )?;
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
            tags: input.tags.clone(),
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
        sqlx::query("UPDATE cognition_objects SET current_revision_id=$3,object_epoch=object_epoch+1,integrity_state='valid' WHERE object_kind='memory' AND subject_id=$1 AND object_id=$2").bind(input.subject.0).bind(input.memory_id.0).bind(revision_id.0).execute(&mut **tx).await.map_err(db)?;
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
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "a relation cannot target itself",
            ));
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
            .start_mutation(
                ErrorOwner::Memory,
                subject,
                operation_id,
                "link_memory_revisions",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        sqlx::query("INSERT INTO memory_revision_relations(from_revision_id,to_revision_id,relation,created_at) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(from.0).bind(to.0).bind(relation.as_str()).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        mutation
            .invalidate(ProjectionInvalidation::topology())
            .await?;

        mutation.commit(&[]).await
    }
}
