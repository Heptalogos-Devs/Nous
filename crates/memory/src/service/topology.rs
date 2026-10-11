// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;

mod validation;
use validation::{association_basis_parts, parse_use_event_key, validate_association_input};

impl MemoryService {
    pub async fn create_association(
        &self,
        mut input: CreateAssociationRequest,
        subject: SubjectId,
    ) -> Result<AssociationEvidence> {
        input.producer = input
            .producer
            .as_ref()
            .map(AuthorityStore::canonical_producer)
            .transpose()?;
        validate_association_input(&input)?;
        let digest = operation_digest(
            "create_association",
            subject,
            &serde_json::json!({"from":input.from,"to":input.to,"relation_kind":input.relation_kind,"polarity":input.polarity,"basis_class":input.basis_class,"basis":input.basis,"producer_signature_id":input.producer_signature_id,"producer":input.producer,"valid_time":input.valid_time}),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Concept,
                subject,
                input.operation_id,
                "create_association",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                let id = receipt
                    .results
                    .iter()
                    .find_map(|result| match &result.reference {
                        CognitiveRef::Association(id) => Some(*id),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        Error::new(
                            ErrorOwner::Concept,
                            ErrorCode::InternalFailure,
                            "association receipt missing result",
                        )
                    })?;
                return self.association(subject, id).await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        self.store.validate_reference(subject, &input.from).await?;
        self.store.validate_reference(subject, &input.to).await?;
        self.validate_association_basis(subject, &input).await?;
        if input.producer.is_some() {
            self.require_current_concept_endpoint(mutation.tx(), subject, &input.from)
                .await?;
            self.require_current_concept_endpoint(mutation.tx(), subject, &input.to)
                .await?;
        }
        if let Some(producer) = &input.producer {
            let registered = AuthorityStore::register_producer_in(mutation.tx(), producer).await?;
            if input
                .producer_signature_id
                .is_some_and(|id| id != registered)
            {
                return Err(Error::new(
                    ErrorOwner::Concept,
                    ErrorCode::InvalidRequest,
                    "association producer identity mismatch",
                ));
            }
            input.producer_signature_id = Some(registered);
        }
        let id = self
            .insert_association_in(mutation.tx(), subject, &input)
            .await?;
        mutation
            .invalidate(ProjectionInvalidation::topology())
            .await?;
        mutation
            .commit(&[MutationResult::reference(CognitiveRef::Association(id))])
            .await?;
        self.association(subject, id).await
    }

    async fn require_current_concept_endpoint(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<()> {
        let (kind, id) = match reference {
            CognitiveRef::MemoryRevision(_)
            | CognitiveRef::CognitiveSchemaRevision(_)
            | CognitiveRef::EpisodeRevision(_)
            | CognitiveRef::JournalRevision(_) => {
                let (kind, id, _) = CognitionKind::from_reference(reference)?;
                (kind, id)
            }
            CognitiveRef::Tag(id) => {
                let active: Option<Uuid> = sqlx::query_scalar("SELECT tag_id FROM tags WHERE subject_id=$1 AND tag_id=$2 AND status='active' FOR SHARE")
                    .bind(subject.0).bind(id.0).fetch_optional(&mut **tx).await.map_err(db)?;
                return active.map(|_| ()).ok_or_else(|| {
                    nous_core::Error::new(
                        ErrorOwner::Concept,
                        ErrorCode::StaleRevision,
                        "concept Tag endpoint is stale",
                    )
                });
            }
            _ => return Ok(()),
        };
        let current: Option<Uuid> = sqlx::query_scalar("SELECT current_revision_id FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND current_revision_id=$3 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' AND purge_state='normal' FOR SHARE")
            .bind(subject.0)
            .bind(kind.as_str())
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?;
        if current != Some(id) {
            return Err(nous_core::Error::new(
                ErrorOwner::Concept,
                ErrorCode::StaleRevision,
                "concept cognition endpoint is stale",
            ));
        }
        Ok(())
    }

    pub(crate) async fn insert_association_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        input: &CreateAssociationRequest,
    ) -> Result<AssociationEvidenceId> {
        let id = AssociationEvidenceId::new();
        let now = self.cognition.now(subject);
        let (valid_kind, valid_start, valid_end) = temporal_columns(&input.valid_time);
        let (from_kind, from_ref) = reference_parts(&input.from);
        let (to_kind, to_ref) = reference_parts(&input.to);
        sqlx::query("INSERT INTO association_evidence(association_evidence_id,subject_id,from_ref_kind,from_ref,to_ref_kind,to_ref,relation_kind,polarity,basis_class,valid_time_kind,valid_time_start,valid_time_end,producer_signature_id,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)").bind(id.0).bind(subject.0).bind(from_kind).bind(from_ref).bind(to_kind).bind(to_ref).bind(&input.relation_kind).bind(input.polarity.as_str()).bind(input.basis_class.as_str()).bind(valid_kind).bind(valid_start).bind(valid_end).bind(input.producer_signature_id).bind(now).execute(&mut **tx).await.map_err(db)?;
        for basis in &input.basis {
            let (
                kind,
                value,
                role,
                occurrence,
                source_region,
                derived_representation,
                derived_region,
            ) = association_basis_parts(basis)?;
            sqlx::query("INSERT INTO association_evidence_basis(association_evidence_id,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                .bind(id.0)
                .bind(kind)
                .bind(value)
                .bind(role)
                .bind(occurrence)
                .bind(source_region)
                .bind(derived_representation)
                .bind(derived_region)
                .bind(epistemic_relation_text(basis.epistemic_relation()))
                .execute(&mut **tx)
                .await
                .map_err(db)?;
        }
        self.store
            .ensure_identity_addresses_in(tx, subject, &[CognitiveRef::Association(id)], "")
            .await?;
        Ok(id)
    }
    /// Bounded undirected read of active, supported AssociationEvidence.
    pub async fn association_neighborhood(
        &self,
        subject: SubjectId,
        root: CognitiveRef,
        max_nodes: usize,
        max_depth: usize,
    ) -> Result<AssociationNeighborhood> {
        if !(1..=256).contains(&max_nodes) || !(1..=4).contains(&max_depth) {
            return Err(Error::new(
                ErrorOwner::Concept,
                ErrorCode::InvalidRequest,
                "neighborhood requires 1..256 nodes and 1..4 depth",
            ));
        }
        self.store.require_subject(subject).await?;
        let (root, _, _) = self.store.bind_exact_reference(subject, &root).await?;
        let mut seen = std::collections::HashSet::from([root.clone()]);
        let mut frontier = vec![root.clone()];
        let mut nodes = vec![root];
        let mut edge_ids = std::collections::HashSet::new();
        let mut associations = Vec::new();
        let mut truncated = false;
        for _ in 0..max_depth {
            if frontier.is_empty() {
                break;
            }
            let (kinds, refs): (Vec<_>, Vec<_>) = frontier.iter().map(reference_parts).unzip();
            let rows = sqlx::query("WITH frontier AS (SELECT * FROM unnest($2::text[], $3::text[]) AS f(kind,ref)), active AS (SELECT association_evidence_id,from_ref_kind,to_ref_kind,CASE WHEN from_ref_kind='tag' THEN canonical_tag($1,from_ref::uuid)::text ELSE from_ref END AS from_ref,CASE WHEN to_ref_kind='tag' THEN canonical_tag($1,to_ref::uuid)::text ELSE to_ref END AS to_ref FROM association_evidence WHERE subject_id=$1 AND revoked_at IS NULL) SELECT a.* FROM active a WHERE a.from_ref IS NOT NULL AND a.to_ref IS NOT NULL AND (a.from_ref_kind<>a.to_ref_kind OR a.from_ref<>a.to_ref) AND EXISTS(SELECT 1 FROM frontier f WHERE (f.kind=a.from_ref_kind AND f.ref=a.from_ref) OR (f.kind=a.to_ref_kind AND f.ref=a.to_ref)) ORDER BY a.association_evidence_id LIMIT 257")
                .bind(subject.0).bind(kinds).bind(refs).fetch_all(self.store.pool()).await.map_err(db)?;
            truncated |= rows.len() > 256;
            let mut next = Vec::new();
            for row in rows.into_iter().take(256) {
                let id: Uuid = row.try_get("association_evidence_id").map_err(db)?;
                if edge_ids.contains(&id) {
                    continue;
                }
                let from = parse_reference(
                    &row.try_get::<String, _>("from_ref_kind").map_err(db)?,
                    &row.try_get::<String, _>("from_ref").map_err(db)?,
                )?;
                let to = parse_reference(
                    &row.try_get::<String, _>("to_ref_kind").map_err(db)?,
                    &row.try_get::<String, _>("to_ref").map_err(db)?,
                )?;
                let added = [&from, &to]
                    .iter()
                    .filter(|reference| !seen.contains(*reference))
                    .count();
                if nodes.len() + added > max_nodes || associations.len() == 256 {
                    truncated = true;
                    continue;
                }
                let mut association = self.association(subject, AssociationEvidenceId(id)).await?;
                association.from = from.clone();
                association.to = to.clone();
                for reference in [from, to]
                    .into_iter()
                    .filter(|reference| seen.insert(reference.clone()))
                {
                    nodes.push(reference.clone());
                    next.push(reference);
                }
                edge_ids.insert(id);
                associations.push(association);
            }
            frontier = next;
        }
        Ok(AssociationNeighborhood {
            nodes,
            associations,
            truncated,
        })
    }

    pub(crate) async fn association(
        &self,
        subject: SubjectId,
        id: AssociationEvidenceId,
    ) -> Result<AssociationEvidence> {
        let row = sqlx::query(
            "SELECT * FROM association_evidence WHERE subject_id=$1 AND association_evidence_id=$2",
        )
        .bind(subject.0)
        .bind(id.0)
        .fetch_optional(self.store.pool())
        .await
        .map_err(db)?
        .ok_or_else(|| {
            Error::new(
                ErrorOwner::Concept,
                ErrorCode::NotFound,
                "association evidence not found",
            )
        })?;
        let from = parse_reference(
            &row.try_get::<String, _>("from_ref_kind").map_err(db)?,
            &row.try_get::<String, _>("from_ref").map_err(db)?,
        )?;
        let to = parse_reference(
            &row.try_get::<String, _>("to_ref_kind").map_err(db)?,
            &row.try_get::<String, _>("to_ref").map_err(db)?,
        )?;
        let basis=sqlx::query("SELECT basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation FROM association_evidence_basis WHERE association_evidence_id=$1 ORDER BY basis_kind,basis_ref,basis_role").bind(id.0).fetch_all(self.store.pool()).await.map_err(db)?.into_iter().map(|row| {
            let kind: String = row.try_get("basis_kind").map_err(db)?;
            if kind == "use_event" {
                parse_enum::<BasisRole>(ErrorOwner::Concept, row.try_get("basis_role").map_err(db)?, "association basis role")?;
                let value = row.try_get::<String, _>("basis_ref").map_err(db)?;
                let (consumer_ref, event_id) = parse_use_event_key(&value)?;
                Ok(AssociationBasis::UseEvent(UseEventRef {
                    subject_id: subject,
                    consumer_ref,
                    event_id: UseEventId(event_id),
                }))
            } else {
                Ok(AssociationBasis::Revision(decode_revision_basis(ErrorOwner::Concept, &row)?))
            }
        }).collect::<Result<Vec<_>>>()?;
        Ok(AssociationEvidence {
            association_evidence_id: id,
            subject_id: subject,
            from,
            to,
            relation_kind: row.try_get("relation_kind").map_err(db)?,
            polarity: parse_enum(
                ErrorOwner::Concept,
                row.try_get("polarity").map_err(db)?,
                "association polarity",
            )?,
            basis_class: parse_enum(
                ErrorOwner::Concept,
                row.try_get("basis_class").map_err(db)?,
                "association support class",
            )?,
            basis,
            producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
            valid_time: temporal_from_columns(
                ErrorOwner::Concept,
                row.try_get("valid_time_kind").map_err(db)?,
                row.try_get("valid_time_start").map_err(db)?,
                row.try_get("valid_time_end").map_err(db)?,
            )?,
            created_at: row.try_get("created_at").map_err(db)?,
            revoked_at: row.try_get("revoked_at").map_err(db)?,
        })
    }

    async fn validate_association_basis(
        &self,
        subject: SubjectId,
        input: &CreateAssociationRequest,
    ) -> Result<()> {
        let mut has_source_evidence = false;
        let mut use_events = Vec::new();
        for basis in &input.basis {
            match basis {
                AssociationBasis::Revision(value) => {
                    if matches!(value, RevisionBasis::Evidence(_)) {
                        has_source_evidence = true;
                    }
                    self.validate_basis_for_subject(
                        ErrorOwner::Concept,
                        subject,
                        std::slice::from_ref(value),
                    )
                    .await?;
                }
                AssociationBasis::UseEvent(value) => {
                    if value.subject_id != subject {
                        return Err(Error::new(
                            ErrorOwner::Concept,
                            ErrorCode::InvalidRequest,
                            "association UseEvent support belongs to another Subject",
                        ));
                    }
                    let active = sqlx::query_scalar::<_, String>(
                        "SELECT use_kind FROM cognitive_use_events WHERE subject_id=$1 AND consumer_ref=$2 AND event_id=$3",
                    )
                    .bind(subject.0)
                    .bind(&value.consumer_ref)
                    .bind(value.event_id.0)
                    .fetch_optional(self.store.pool())
                    .await
                    .map_err(db)?;
                    let purged = sqlx::query_scalar::<_, bool>(
                        "SELECT EXISTS(SELECT 1 FROM purged_use_receipts WHERE subject_id=$1 AND consumer_ref=$2 AND event_id=$3)",
                    )
                    .bind(subject.0)
                    .bind(&value.consumer_ref)
                    .bind(value.event_id.0)
                    .fetch_one(self.store.pool())
                    .await
                    .map_err(db)?;
                    if active.is_none() && !purged {
                        return Err(Error::new(
                            ErrorOwner::Concept,
                            ErrorCode::InvalidRequest,
                            "association UseEvent support was not recorded",
                        ));
                    }
                    use_events.push(active);
                }
            }
        }
        match input.basis_class {
            AssociationBasisClass::HostExplicit => {}
            AssociationBasisClass::SourceEvidence if !has_source_evidence => {
                return Err(Error::new(
                    ErrorOwner::Concept,
                    ErrorCode::InvalidRequest,
                    "source_evidence association needs an EvidenceRef support",
                ));
            }
            AssociationBasisClass::CognitiveDerivation
            | AssociationBasisClass::DerivedStructure => {
                if let Some(producer) = &input.producer {
                    if producer.operation != CapabilityOperation::ConceptMaintenanceText {
                        return Err(Error::new(
                            ErrorOwner::Concept,
                            ErrorCode::InvalidRequest,
                            "association producer operation mismatch",
                        ));
                    }
                    serde_json::from_value::<TopologyRelation>(serde_json::json!(
                        input.relation_kind
                    ))
                    .map_err(|error| {
                        Error::new(
                            ErrorOwner::Concept,
                            ErrorCode::InvalidRequest,
                            "unregistered concept relation",
                        )
                        .with_source(error)
                    })?;
                } else {
                    let producer = input.producer_signature_id.ok_or_else(|| {
                        Error::new(
                            ErrorOwner::Concept,
                            ErrorCode::InvalidRequest,
                            "derived association requires producer provenance",
                        )
                    })?;
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM producer_signatures WHERE producer_signature_id=$1)")
                        .bind(producer).fetch_one(self.store.pool()).await.map_err(db)?;
                    if !exists {
                        return Err(Error::new(
                            ErrorOwner::Concept,
                            ErrorCode::InvalidRequest,
                            "producer signature is not registered",
                        ));
                    }
                }
                if input
                    .basis
                    .iter()
                    .all(|basis| matches!(basis, AssociationBasis::UseEvent(_)))
                {
                    return Err(Error::new(
                        ErrorOwner::Concept,
                        ErrorCode::InvalidRequest,
                        "derived association needs revision provenance support",
                    ));
                }
            }
            AssociationBasisClass::MeaningfulUse if use_events.is_empty() => {
                return Err(Error::new(
                    ErrorOwner::Concept,
                    ErrorCode::InvalidRequest,
                    "meaningful_use association needs a UseEventRef support",
                ));
            }
            AssociationBasisClass::MeaningfulUse => {
                if use_events.iter().any(Option::is_none) {
                    return Err(Error::new(
                        ErrorOwner::Concept,
                        ErrorCode::InvalidRequest,
                        "purged UseEvent receipts cannot prove meaningful-use kind",
                    ));
                }
                for use_kind in use_events.into_iter().flatten() {
                    let allowed = matches!(
                        use_kind.as_str(),
                        "referenced" | "acted_on" | "result_supported" | "corrected" | "pinned"
                    ) || (input.polarity == AssociationPolarity::Negative
                        && use_kind == "result_refuted");
                    if !allowed {
                        return Err(Error::new(
                            ErrorOwner::Concept,
                            ErrorCode::InvalidRequest,
                            "UseEvent kind cannot support a positive meaningful-use association",
                        ));
                    }
                }
            }
            AssociationBasisClass::SourceEvidence => {}
        }
        Ok(())
    }

    pub async fn revoke_association(
        &self,
        subject: SubjectId,
        association: AssociationEvidenceId,
        operation_id: OperationId,
    ) -> Result<()> {
        let digest = operation_digest(
            "revoke_association",
            subject,
            &serde_json::json!({"association_id": association}),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Concept,
                subject,
                operation_id,
                "revoke_association",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let changed = sqlx::query("UPDATE association_evidence SET revoked_at=COALESCE(revoked_at,$3) WHERE subject_id=$1 AND association_evidence_id=$2")
            .bind(subject.0)
            .bind(association.0)
            .bind(self.cognition.now(subject))
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        if changed.rows_affected() == 0 {
            return Err(Error::new(
                ErrorOwner::Concept,
                ErrorCode::NotFound,
                "association evidence not found",
            ));
        }
        mutation
            .invalidate(ProjectionInvalidation::topology())
            .await?;
        mutation
            .commit(&[MutationResult::reference(CognitiveRef::Association(
                association,
            ))])
            .await
    }

    pub async fn accessibility_level(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        now: DateTime<Utc>,
    ) -> Result<AccessibilityLevel> {
        let policy = super::resolve_accessibility_policy(
            &self.configuration.snapshot_for_subject(subject)?,
        )?;
        policy
            .level_for_memory(&self.store, subject, memory, now)
            .await
    }

    pub async fn temporal_evidence_public(
        &self,
        revision: MemoryRevisionId,
    ) -> Result<TemporalEvidence> {
        self.temporal_evidence(revision).await
    }

    pub async fn require_exact_revision(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<()> {
        if !matches!(
            reference,
            CognitiveRef::MemoryRevision(_) | CognitiveRef::CognitiveSchemaRevision(_)
        ) {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "durable cognition reference must be an exact revision",
            ));
        }
        self.store.validate_reference(subject, reference).await
    }
}
