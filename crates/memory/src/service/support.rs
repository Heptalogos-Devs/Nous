//! Exact cognition support is read in batches within the owner's captured read.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::HashMap;

pub(super) struct CognitionSupport {
    pub object: CognitiveRef,
    pub basis: Vec<RevisionBasis>,
    pub members: Vec<EpisodeMember>,
}

impl MemoryService {
    pub(super) async fn exact_supports(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, CognitionSupport>> {
        let mut tx = self.store.read_view().await?;
        let result = self
            .exact_supports_in(&mut tx, subject, references, view)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }

    pub(super) async fn exact_supports_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, CognitionSupport>> {
        if references.len() > 256 {
            return Err(Error::Invalid(
                "cognition support batch exceeds 256 references".into(),
            ));
        }
        if references.is_empty() {
            return Ok(HashMap::new());
        }
        let mut selected: HashMap<&str, Vec<Uuid>> = HashMap::new();
        for reference in references {
            if view.is_some_and(|v| v.subject != subject || v.cognition_for(reference).is_none()) {
                return Err(Error::NotFound(
                    "cognition support is outside the captured view".into(),
                ));
            }
            if !matches!(
                reference,
                CognitiveRef::MemoryRevision(_)
                    | CognitiveRef::CognitiveSchemaRevision(_)
                    | CognitiveRef::EpisodeRevision(_)
                    | CognitiveRef::JournalRevision(_)
            ) {
                return Err(Error::Invalid(
                    "cognition dependency needs exact revision".into(),
                ));
            }
            selected
                .entry(reference.kind())
                .or_default()
                .push(reference.uuid().expect("exact revision UUID"));
        }
        let ids = |kind| selected.get(kind).map_or(&[][..], Vec::as_slice);
        let rows = sqlx::query("SELECT 'memory_revision' kind,r.memory_revision_id id,r.memory_id object_id FROM memory_revisions r WHERE r.subject_id=$1 AND r.memory_revision_id=ANY($2) UNION ALL SELECT 'cognitive_schema_revision',r.schema_revision_id,r.schema_id FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=ANY($3) UNION ALL SELECT 'episode_revision',r.episode_revision_id,r.episode_id FROM episode_revisions r WHERE r.subject_id=$1 AND r.episode_revision_id=ANY($4) UNION ALL SELECT 'journal_revision',r.journal_revision_id,r.journal_id FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE o.subject_id=$1 AND r.journal_revision_id=ANY($5)")
            .bind(subject.0).bind(ids("memory_revision")).bind(ids("cognitive_schema_revision")).bind(ids("episode_revision")).bind(ids("journal_revision")).fetch_all(&mut **tx).await.map_err(db)?;
        let mut supports = HashMap::new();
        for row in rows {
            let kind: String = row.try_get("kind").map_err(db)?;
            let id: Uuid = row.try_get("id").map_err(db)?;
            let owner: Uuid = row.try_get("object_id").map_err(db)?;
            let (reference, object) = match kind.as_str() {
                "memory_revision" => (
                    CognitiveRef::MemoryRevision(MemoryRevisionId(id)),
                    CognitiveRef::Memory(MemoryId(owner)),
                ),
                "cognitive_schema_revision" => (
                    CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(id)),
                    CognitiveRef::CognitiveSchema(CognitiveSchemaId(owner)),
                ),
                "episode_revision" => (
                    CognitiveRef::EpisodeRevision(EpisodeRevisionId(id)),
                    CognitiveRef::Episode(EpisodeId(owner)),
                ),
                "journal_revision" => (
                    CognitiveRef::JournalRevision(JournalRevisionId(id)),
                    CognitiveRef::Journal(JournalId(owner)),
                ),
                _ => unreachable!("owner support query returns four families"),
            };
            supports.insert(
                reference,
                CognitionSupport {
                    object,
                    basis: vec![],
                    members: vec![],
                },
            );
        }
        if references.iter().any(|r| !supports.contains_key(r)) {
            return Err(Error::Invalid(
                "cognition dependency is outside Subject".into(),
            ));
        }
        if let Some(ids) = selected.get("memory_revision") {
            for (id, basis) in self.memory_basis_in(tx, ids).await? {
                supports
                    .get_mut(&CognitiveRef::MemoryRevision(MemoryRevisionId(id)))
                    .expect("selected Memory")
                    .basis = basis;
            }
        }
        if let Some(ids) = selected.get("cognitive_schema_revision") {
            let rows = sqlx::query("SELECT * FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND schema_revision_id=ANY($2) AND (($3 AND link_id=ANY($4)) OR (NOT $3 AND revoked_at IS NULL)) ORDER BY schema_revision_id,link_id")
                .bind(subject.0).bind(ids).bind(view.is_some()).bind(view.map(|v| v.schema_evidence_links.as_slice()).unwrap_or_default()).fetch_all(&mut **tx).await.map_err(db)?;
            for row in rows {
                let id = CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(
                    row.try_get("schema_revision_id").map_err(db)?,
                ));
                supports
                    .get_mut(&id)
                    .expect("selected Schema")
                    .basis
                    .push(basis_from_row(&row)?);
            }
        }
        if let Some(ids) = selected.get("episode_revision") {
            let (basis, members) = self.episode_supports_in(tx, ids).await?;
            for (id, basis) in basis {
                supports
                    .get_mut(&CognitiveRef::EpisodeRevision(EpisodeRevisionId(id)))
                    .expect("selected Episode")
                    .basis = basis;
            }
            for (id, members) in members {
                supports
                    .get_mut(&CognitiveRef::EpisodeRevision(EpisodeRevisionId(id)))
                    .expect("selected Episode")
                    .members = members;
            }
        }
        if let Some(ids) = selected.get("journal_revision") {
            for row in sqlx::query("SELECT journal_revision_id,basis FROM journal_point_basis WHERE journal_revision_id=ANY($1) ORDER BY journal_revision_id,ordinal,basis_no").bind(ids).fetch_all(&mut **tx).await.map_err(db)? {
                let id = CognitiveRef::JournalRevision(JournalRevisionId(row.try_get("journal_revision_id").map_err(db)?));
                supports.get_mut(&id).expect("selected Journal").basis.push(serde_json::from_value(row.try_get("basis").map_err(db)?).map_err(|e| Error::Infrastructure(e.to_string()))?);
            }
        }
        Ok(supports)
    }

    pub(super) async fn memory_basis_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        revisions: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<RevisionBasis>>> {
        let rows = sqlx::query("SELECT memory_revision_id,'evidence'::text basis_kind,NULL::text basis_ref,occurrence_id,source_region_id,derived_representation_id,derived_region_id,basis_role,epistemic_relation,0 family,evidence_no ordinal FROM memory_revision_evidence WHERE memory_revision_id=ANY($1) UNION ALL SELECT memory_revision_id,target_ref_kind,target_ref,NULL::uuid,NULL::uuid,NULL::uuid,NULL::uuid,basis_role,epistemic_relation,1,0 FROM memory_revision_dependencies WHERE memory_revision_id=ANY($1) ORDER BY memory_revision_id,family,ordinal,basis_kind,basis_ref")
            .bind(revisions).fetch_all(&mut **tx).await.map_err(db)?;
        let mut result: HashMap<Uuid, Vec<RevisionBasis>> = HashMap::new();
        for row in rows {
            result
                .entry(row.try_get("memory_revision_id").map_err(db)?)
                .or_default()
                .push(basis_from_row(&row)?);
        }
        Ok(result)
    }
}

pub(super) fn basis_from_row(row: &sqlx::postgres::PgRow) -> Result<RevisionBasis> {
    let role = parse_enum(row.try_get("basis_role").map_err(db)?, "basis role")?;
    let relation = parse_epistemic_relation(row.try_get("epistemic_relation").map_err(db)?)?;
    let kind: String = row.try_get("basis_kind").map_err(db)?;
    if kind == "evidence" {
        let locator = match (
            row.try_get::<Option<Uuid>, _>("source_region_id")
                .map_err(db)?,
            row.try_get::<Option<Uuid>, _>("derived_representation_id")
                .map_err(db)?,
            row.try_get::<Option<Uuid>, _>("derived_region_id")
                .map_err(db)?,
        ) {
            (Some(id), None, None) => EvidenceLocator::SourceRegion(SourceRegionId(id)),
            (None, Some(id), None) => {
                EvidenceLocator::DerivedRepresentation(DerivedRepresentationId(id))
            }
            (None, None, Some(id)) => EvidenceLocator::DerivedRegion(DerivedRegionId(id)),
            (None, None, None) => EvidenceLocator::WholeOccurrence,
            _ => {
                return Err(Error::Infrastructure(
                    "evidence locator union is invalid".into(),
                ));
            }
        };
        Ok(RevisionBasis::Evidence(EvidenceRef {
            occurrence_id: OccurrenceId(row.try_get("occurrence_id").map_err(db)?),
            locator,
            basis_role: role,
            epistemic_relation: relation,
        }))
    } else {
        Ok(RevisionBasis::CognitionDependency(CognitionDependency {
            target_revision: parse_reference(
                &kind,
                &row.try_get::<String, _>("basis_ref").map_err(db)?,
            )?,
            basis_role: role,
            epistemic_relation: relation,
        }))
    }
}
