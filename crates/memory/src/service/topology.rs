// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;

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
            .start_mutation(subject, input.operation_id, "create_association", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                let id = receipt.result_ref;

                return self
                    .association(
                        subject,
                        AssociationEvidenceId(
                            id.ok_or_else(|| {
                                Error::Infrastructure("association receipt missing result".into())
                            })?
                            .parse()
                            .map_err(|_| {
                                Error::Infrastructure("invalid association receipt".into())
                            })?,
                        ),
                    )
                    .await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        for reference in [&input.from, &input.to] {
            if !self
                .store
                .reference_in_subject_tx(mutation.tx(), subject, reference)
                .await?
            {
                return Err(Error::NotFound("association endpoint not found".into()));
            }
        }
        self.validate_association_basis(mutation.tx(), subject, &input)
            .await?;
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
                return Err(Error::Invalid(
                    "association producer identity mismatch".into(),
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
            .publish_workflow_results("memory", &[CognitiveRef::Association(id)])
            .await?;
        mutation
            .commit("association", Some(&id.0.to_string()), None, None)
            .await?;
        self.association(subject, id).await
    }

    async fn require_current_concept_endpoint(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<()> {
        let (revision_table, object_table, revision_column, object_column, id) = match reference {
            CognitiveRef::MemoryRevision(id) => (
                "memory_revisions",
                "memory_objects",
                "memory_revision_id",
                "memory_id",
                id.0,
            ),
            CognitiveRef::CognitiveSchemaRevision(id) => (
                "cognitive_schema_revisions",
                "cognitive_schemas",
                "schema_revision_id",
                "schema_id",
                id.0,
            ),
            CognitiveRef::EpisodeRevision(id) => (
                "episode_revisions",
                "episode_objects",
                "episode_revision_id",
                "episode_id",
                id.0,
            ),
            CognitiveRef::JournalRevision(id) => (
                "journal_revisions",
                "journal_objects",
                "journal_revision_id",
                "journal_id",
                id.0,
            ),
            CognitiveRef::Tag(id) => {
                let active: Option<Uuid> = sqlx::query_scalar("SELECT tag_id FROM tags WHERE subject_id=$1 AND tag_id=$2 AND status='active' FOR SHARE")
                    .bind(subject.0).bind(id.0).fetch_optional(&mut **tx).await.map_err(db)?;
                return active.map(|_| ()).ok_or_else(|| {
                    nous_core::DomainError::new(
                        nous_core::DomainErrorCode::StaleRevision,
                        "concept Tag endpoint is stale",
                    )
                    .into()
                });
            }
            _ => return Ok(()),
        };
        let mut query =
            sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT o.current_revision_id FROM ");
        query.push(revision_table).push(" r JOIN ").push(object_table)
            .push(" o USING(").push(object_column).push(") WHERE o.subject_id=")
            .push_bind(subject.0).push(" AND r.").push(revision_column).push("=")
            .push_bind(id).push(" AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' FOR SHARE OF o");
        let current: Option<Uuid> = query
            .build_query_scalar()
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?;
        if current != Some(id) {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "concept cognition endpoint is stale",
            )
            .into());
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
        sqlx::query("INSERT INTO association_evidence(association_evidence_id,subject_id,from_ref_kind,from_ref,to_ref_kind,to_ref,relation_kind,polarity,basis_class,valid_time_kind,valid_time_start,valid_time_end,producer_signature_id,created_at,valid_time_domain_ref) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)").bind(id.0).bind(subject.0).bind(from_kind).bind(from_ref).bind(to_kind).bind(to_ref).bind(&input.relation_kind).bind(input.polarity.as_str()).bind(input.basis_class.as_str()).bind(valid_kind).bind(valid_start).bind(valid_end).bind(input.producer_signature_id).bind(now).bind(&input.valid_time_domain_ref).execute(&mut **tx).await.map_err(db)?;
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
            return Err(Error::Invalid(
                "neighborhood requires 1..256 nodes and 1..4 depth".into(),
            ));
        }
        let mut tx = self.store.read_view().await?;
        let (root, _, _) = self
            .store
            .bind_exact_reference_in(&mut tx, subject, &root)
            .await?;
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
            let rows = sqlx::query("WITH frontier AS (SELECT * FROM unnest($2::text[], $3::text[]) AS f(kind,ref)), active AS (SELECT a.*,CASE WHEN from_ref_kind='tag' THEN canonical_tag($1,from_ref::uuid)::text ELSE from_ref END AS effective_from,CASE WHEN to_ref_kind='tag' THEN canonical_tag($1,to_ref::uuid)::text ELSE to_ref END AS effective_to FROM association_evidence a WHERE subject_id=$1 AND revoked_at IS NULL) SELECT a.* FROM active a WHERE a.effective_from IS NOT NULL AND a.effective_to IS NOT NULL AND (a.from_ref_kind<>a.to_ref_kind OR a.effective_from<>a.effective_to) AND EXISTS(SELECT 1 FROM frontier f WHERE (f.kind=a.from_ref_kind AND f.ref=a.effective_from) OR (f.kind=a.to_ref_kind AND f.ref=a.effective_to)) ORDER BY a.association_evidence_id LIMIT 257")
                .bind(subject.0).bind(kinds).bind(refs).fetch_all(&mut *tx).await.map_err(db)?;
            truncated |= rows.len() > 256;
            let mut next = Vec::new();
            for row in rows.into_iter().take(256) {
                let id: Uuid = row.try_get("association_evidence_id").map_err(db)?;
                if edge_ids.contains(&id) {
                    continue;
                }
                let from = parse_reference(
                    &row.try_get::<String, _>("from_ref_kind").map_err(db)?,
                    &row.try_get::<String, _>("effective_from").map_err(db)?,
                )?;
                let to = parse_reference(
                    &row.try_get::<String, _>("to_ref_kind").map_err(db)?,
                    &row.try_get::<String, _>("effective_to").map_err(db)?,
                )?;
                let added = [&from, &to]
                    .iter()
                    .filter(|reference| !seen.contains(*reference))
                    .count();
                if nodes.len() + added > max_nodes || associations.len() == 256 {
                    truncated = true;
                    continue;
                }
                let association =
                    decode_association(&row, subject, from.clone(), to.clone(), Vec::new())?;
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
        let ids = associations
            .iter()
            .map(|edge| edge.association_evidence_id.0)
            .collect::<Vec<_>>();
        let mut basis = self.association_basis_in(&mut tx, subject, &ids).await?;
        for association in &mut associations {
            association.basis = basis
                .remove(&association.association_evidence_id.0)
                .unwrap_or_default();
        }
        tx.commit().await.map_err(db)?;
        Ok(AssociationNeighborhood {
            nodes,
            associations,
            truncated,
        })
    }

    pub async fn association_page(
        &self,
        subject: SubjectId,
        status: &str,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<AssociationEvidence>, bool)> {
        let status = if status.is_empty() { "active" } else { status };
        if !(1..=200).contains(&limit) || !matches!(status, "active" | "revoked") {
            return Err(Error::Invalid("invalid Association page envelope".into()));
        }
        self.store.require_subject(subject).await?;
        let mut tx = self.store.read_view().await?;
        let mut rows = sqlx::query("SELECT * FROM association_evidence WHERE subject_id=$1 AND (revoked_at IS NULL)=($2='active') AND ($3::uuid IS NULL OR association_evidence_id>$3) ORDER BY association_evidence_id LIMIT $4")
            .bind(subject.0).bind(status).bind(after).bind(limit+1).fetch_all(&mut *tx).await.map_err(db)?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let ids = rows
            .iter()
            .map(|r| r.try_get::<Uuid, _>("association_evidence_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        let mut basis = self.association_basis_in(&mut tx, subject, &ids).await?;
        let items = rows
            .into_iter()
            .map(|row| {
                let id: Uuid = row.try_get("association_evidence_id").map_err(db)?;
                let from = parse_reference(
                    &row.try_get::<String, _>("from_ref_kind").map_err(db)?,
                    &row.try_get::<String, _>("from_ref").map_err(db)?,
                )?;
                let to = parse_reference(
                    &row.try_get::<String, _>("to_ref_kind").map_err(db)?,
                    &row.try_get::<String, _>("to_ref").map_err(db)?,
                )?;
                decode_association(
                    &row,
                    subject,
                    from,
                    to,
                    basis.remove(&id).unwrap_or_default(),
                )
            })
            .collect::<Result<_>>()?;
        tx.commit().await.map_err(db)?;
        Ok((items, more))
    }
    pub(super) async fn association_basis_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, Vec<AssociationBasis>>> {
        let mut basis = std::collections::HashMap::<Uuid, Vec<AssociationBasis>>::new();
        for row in sqlx::query("SELECT * FROM association_evidence_basis WHERE association_evidence_id=ANY($1::uuid[]) ORDER BY association_evidence_id,basis_kind,basis_ref,basis_role")
            .bind(ids).fetch_all(&mut **tx).await.map_err(db)? {
            basis.entry(row.try_get("association_evidence_id").map_err(db)?).or_default().push(decode_association_basis(&row,subject)?);
        }
        Ok(basis)
    }
    pub async fn association(
        &self,
        subject: SubjectId,
        id: AssociationEvidenceId,
    ) -> Result<AssociationEvidence> {
        let mut tx = self.store.read_view().await?;
        let row = sqlx::query(
            "SELECT * FROM association_evidence WHERE subject_id=$1 AND association_evidence_id=$2",
        )
        .bind(subject.0)
        .bind(id.0)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| Error::NotFound("association evidence not found".into()))?;
        let from = parse_reference(
            &row.try_get::<String, _>("from_ref_kind").map_err(db)?,
            &row.try_get::<String, _>("from_ref").map_err(db)?,
        )?;
        let to = parse_reference(
            &row.try_get::<String, _>("to_ref_kind").map_err(db)?,
            &row.try_get::<String, _>("to_ref").map_err(db)?,
        )?;
        let basis = self
            .association_basis_in(&mut tx, subject, &[id.0])
            .await?
            .remove(&id.0)
            .unwrap_or_default();
        let view = decode_association(&row, subject, from, to, basis)?;
        tx.commit().await.map_err(db)?;
        Ok(view)
    }

    async fn validate_association_basis(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        input: &CreateAssociationRequest,
    ) -> Result<()> {
        let mut has_source_evidence = false;
        let mut use_events = Vec::new();
        let mut revision_basis = Vec::new();
        for basis in &input.basis {
            match basis {
                AssociationBasis::Revision(value) => {
                    if matches!(value, RevisionBasis::Evidence(_)) {
                        has_source_evidence = true;
                    }
                    revision_basis.push(value.clone());
                }
                AssociationBasis::UseEvent(value) => {
                    let active = self
                        .cognition
                        .use_event_support_in(tx, subject, value)
                        .await?;
                    use_events.push(active);
                }
            }
        }
        self.provenance_summary_in(tx, subject, &revision_basis, None, &mut Default::default())
            .await?;
        match input.basis_class {
            AssociationBasisClass::HostExplicit => {}
            AssociationBasisClass::SourceEvidence if !has_source_evidence => {
                return Err(Error::Invalid(
                    "source_evidence association needs an EvidenceRef support".into(),
                ));
            }
            AssociationBasisClass::CognitiveDerivation
            | AssociationBasisClass::DerivedStructure => {
                if let Some(producer) = &input.producer {
                    if producer.operation != CapabilityOperation::ConceptMaintenanceText {
                        return Err(Error::Invalid(
                            "association producer operation mismatch".into(),
                        ));
                    }
                    serde_json::from_value::<TopologyRelation>(serde_json::json!(
                        input.relation_kind
                    ))
                    .map_err(|_| Error::Invalid("unregistered concept relation".into()))?;
                } else {
                    let producer = input.producer_signature_id.ok_or_else(|| {
                        Error::Invalid("derived association requires producer provenance".into())
                    })?;
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM producer_signatures WHERE producer_signature_id=$1)")
                        .bind(producer).fetch_one(&mut **tx).await.map_err(db)?;
                    if !exists {
                        return Err(Error::Invalid(
                            "producer signature is not registered".into(),
                        ));
                    }
                }
                if input
                    .basis
                    .iter()
                    .all(|basis| matches!(basis, AssociationBasis::UseEvent(_)))
                {
                    return Err(Error::Invalid(
                        "derived association needs revision provenance support".into(),
                    ));
                }
            }
            AssociationBasisClass::MeaningfulUse if use_events.is_empty() => {
                return Err(Error::Invalid(
                    "meaningful_use association needs a UseEventRef support".into(),
                ));
            }
            AssociationBasisClass::MeaningfulUse => {
                if use_events.iter().any(Option::is_none) {
                    return Err(Error::Invalid(
                        "purged UseEvent receipts cannot prove meaningful-use kind".into(),
                    ));
                }
                for use_kind in use_events.into_iter().flatten() {
                    let allowed = matches!(
                        use_kind.as_str(),
                        "referenced" | "acted_on" | "result_supported" | "corrected" | "pinned"
                    ) || (input.polarity == AssociationPolarity::Negative
                        && use_kind == "result_refuted");
                    if !allowed {
                        return Err(Error::Invalid(
                            "UseEvent kind cannot support a positive meaningful-use association"
                                .into(),
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
            .start_mutation(subject, operation_id, "revoke_association", &digest)
            .await?
        {
            MutationStart::Replay(_) => {
                return Ok(());
            }
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
            return Err(Error::NotFound("association evidence not found".into()));
        }
        mutation
            .invalidate(ProjectionInvalidation::topology())
            .await?;
        mutation
            .commit("association", Some(&association.0.to_string()), None, None)
            .await
    }
}

#[expect(
    clippy::type_complexity,
    reason = "the tuple mirrors the normalized support columns written atomically"
)]
fn association_basis_parts(
    basis: &AssociationBasis,
) -> Result<(
    String,
    String,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
)> {
    match basis {
        AssociationBasis::UseEvent(value) => Ok((
            "use_event".into(),
            value.canonical_key(),
            BasisRole::Contextual.as_str().into(),
            None,
            None,
            None,
            None,
        )),
        AssociationBasis::Revision(RevisionBasis::Evidence(value)) => {
            let (source_region, derived_representation, derived_region) = match value.locator {
                EvidenceLocator::WholeOccurrence => (None, None, None),
                EvidenceLocator::SourceRegion(id) => (Some(id.0), None, None),
                EvidenceLocator::DerivedRepresentation(id) => (None, Some(id.0), None),
                EvidenceLocator::DerivedRegion(id) => (None, None, Some(id.0)),
            };
            Ok((
                "evidence".into(),
                value.canonical_key(),
                value.basis_role.as_str().into(),
                Some(value.occurrence_id.0),
                source_region,
                derived_representation,
                derived_region,
            ))
        }
        AssociationBasis::Revision(RevisionBasis::CognitionDependency(value)) => {
            let (kind, reference) = reference_parts(&value.target_revision);
            Ok((
                kind,
                reference,
                value.basis_role.as_str().into(),
                None,
                None,
                None,
                None,
            ))
        }
        AssociationBasis::Revision(RevisionBasis::Seed(_)) => Err(Error::Invalid(
            "Association support cannot use Cognitive Seed support".into(),
        )),
    }
}

fn validate_association_endpoint(reference: &CognitiveRef) -> Result<()> {
    match reference {
        CognitiveRef::Memory(_) | CognitiveRef::CognitiveSchema(_) => Err(Error::Invalid(
            "association cognition endpoints must be exact revisions".into(),
        )),
        CognitiveRef::MemoryRevision(_)
        | CognitiveRef::CognitiveSchemaRevision(_)
        | CognitiveRef::EpisodeRevision(_)
        | CognitiveRef::JournalRevision(_)
        | CognitiveRef::Entity(_)
        | CognitiveRef::Tag(_)
        | CognitiveRef::Resource(_) => Ok(()),
        _ => Err(Error::Invalid(
            "association endpoint must be cognition revision, Entity, Tag, or Resource".into(),
        )),
    }
}

fn parse_use_event_key(value: &str) -> Result<(String, Uuid)> {
    let (_, rest) = value
        .split_once(':')
        .ok_or_else(|| Error::Infrastructure("invalid association UseEvent key".into()))?;
    let (consumer, event) = rest
        .rsplit_once(':')
        .ok_or_else(|| Error::Infrastructure("invalid association UseEvent key".into()))?;
    let event = event
        .parse()
        .map_err(|_| Error::Infrastructure("invalid association UseEvent id".into()))?;
    Ok((consumer.into(), event))
}

fn validate_association_input(input: &CreateAssociationRequest) -> Result<()> {
    if input.relation_kind.is_empty()
        || input.relation_kind.len() > 128
        || !input
            .relation_kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.:-".contains(&b))
    {
        return Err(Error::Invalid("relation_kind is invalid".into()));
    }
    if !(1..=16).contains(&input.basis.len()) {
        return Err(Error::Invalid("association needs 1..16 basis".into()));
    }
    nous_core::DomainTemporalExtent {
        time_domain_ref: input.valid_time_domain_ref.clone(),
        extent: input.valid_time.clone(),
    }
    .validate()?;
    validate_association_endpoint(&input.from)?;
    validate_association_endpoint(&input.to)?;
    if input.from == input.to {
        return Err(Error::Invalid("association endpoints must differ".into()));
    }
    if input.relation_kind == "tag_attachment" {
        let cognition = |reference: &CognitiveRef| {
            matches!(
                reference,
                CognitiveRef::MemoryRevision(_)
                    | CognitiveRef::CognitiveSchemaRevision(_)
                    | CognitiveRef::EpisodeRevision(_)
                    | CognitiveRef::JournalRevision(_)
            )
        };
        if input.polarity != AssociationPolarity::Positive
            || !(cognition(&input.from) && matches!(input.to, CognitiveRef::Tag(_)))
        {
            return Err(Error::Invalid(
                "tag_attachment requires positive cognition to Tag endpoints".into(),
            ));
        }
    }
    Ok(())
}

fn decode_association_basis(
    row: &sqlx::postgres::PgRow,
    subject: SubjectId,
) -> Result<AssociationBasis> {
    let kind: String = row.try_get("basis_kind").map_err(db)?;
    let basis_role = parse_enum(
        row.try_get("basis_role").map_err(db)?,
        "association basis role",
    )?;
    if kind == "use_event" {
        let value = row.try_get::<String, _>("basis_ref").map_err(db)?;
        let (consumer_ref, event_id) = parse_use_event_key(&value)?;
        Ok(AssociationBasis::UseEvent(UseEventRef {
            subject_id: subject,
            consumer_ref,
            event_id: UseEventId(event_id),
        }))
    } else if kind == "evidence" {
        let locator = match (
            row.try_get::<Option<Uuid>, _>("source_region_id")
                .map_err(db)?,
            row.try_get::<Option<Uuid>, _>("derived_representation_id")
                .map_err(db)?,
            row.try_get::<Option<Uuid>, _>("derived_region_id")
                .map_err(db)?,
        ) {
            (Some(value), None, None) => {
                EvidenceLocator::SourceRegion(nous_core::SourceRegionId(value))
            }
            (None, Some(value), None) => {
                EvidenceLocator::DerivedRepresentation(nous_core::DerivedRepresentationId(value))
            }
            (None, None, Some(value)) => {
                EvidenceLocator::DerivedRegion(nous_core::DerivedRegionId(value))
            }
            (None, None, None) => EvidenceLocator::WholeOccurrence,
            _ => {
                return Err(Error::Infrastructure(
                    "association evidence locator is invalid".into(),
                ));
            }
        };
        Ok(AssociationBasis::Revision(RevisionBasis::Evidence(
            EvidenceRef {
                epistemic_relation: parse_epistemic_relation(
                    row.try_get("epistemic_relation").map_err(db)?,
                )?,
                occurrence_id: OccurrenceId(row.try_get("occurrence_id").map_err(db)?),
                locator,
                basis_role,
            },
        )))
    } else {
        Ok(AssociationBasis::Revision(
            RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: parse_epistemic_relation(
                    row.try_get("epistemic_relation").map_err(db)?,
                )?,
                target_revision: parse_reference(
                    &kind,
                    &row.try_get::<String, _>("basis_ref").map_err(db)?,
                )?,
                basis_role,
            }),
        ))
    }
}

fn decode_association(
    row: &sqlx::postgres::PgRow,
    subject: SubjectId,
    from: CognitiveRef,
    to: CognitiveRef,
    basis: Vec<AssociationBasis>,
) -> Result<AssociationEvidence> {
    Ok(AssociationEvidence {
        valid_time_domain_ref: row.try_get("valid_time_domain_ref").map_err(db)?,
        association_evidence_id: AssociationEvidenceId(
            row.try_get("association_evidence_id").map_err(db)?,
        ),
        subject_id: subject,
        from,
        to,
        relation_kind: row.try_get("relation_kind").map_err(db)?,
        polarity: parse_enum(row.try_get("polarity").map_err(db)?, "association polarity")?,
        basis_class: parse_enum(
            row.try_get("basis_class").map_err(db)?,
            "association support class",
        )?,
        basis,
        producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
        valid_time: temporal_from_columns(
            row.try_get("valid_time_kind").map_err(db)?,
            row.try_get("valid_time_start").map_err(db)?,
            row.try_get("valid_time_end").map_err(db)?,
        )?,
        created_at: row.try_get("created_at").map_err(db)?,
        revoked_at: row.try_get("revoked_at").map_err(db)?,
    })
}
