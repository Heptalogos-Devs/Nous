//! Cognition owns its dependency graph; Material owns the evidence lineage.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

pub enum CognitionContent {
    Memory(MemoryView),
    Schema(super::schema::SchemaView),
    Episode(EpisodeView),
    Journal(JournalView),
}
impl CognitionContent {
    pub fn reference(&self) -> CognitiveRef {
        match self {
            Self::Memory(v) => CognitiveRef::MemoryRevision(v.revision.memory_revision_id),
            Self::Schema(v) => CognitiveRef::CognitiveSchemaRevision(v.revision.schema_revision_id),
            Self::Episode(v) => CognitiveRef::EpisodeRevision(v.revision.episode_revision_id),
            Self::Journal(v) => CognitiveRef::JournalRevision(v.revision.journal_revision_id),
        }
    }
}

impl CognitionContent {
    pub fn trace_sources(&self) -> CognitionTraceSources {
        let (basis, refs, producers, bytes): (Vec<_>, Vec<_>, Vec<_>, _) = match self {
            Self::Memory(v) => (
                v.basis.iter().collect(),
                vec![],
                v.revision.producer_signature_id.into_iter().collect(),
                v.revision.representation_text.len(),
            ),
            Self::Schema(v) => (
                v.evidence_links
                    .iter()
                    .filter(|l| l.revoked_at.is_none())
                    .map(|l| &l.basis)
                    .collect(),
                vec![],
                v.revision
                    .producer_signature_id
                    .into_iter()
                    .chain(
                        v.evidence_links
                            .iter()
                            .filter_map(|l| l.producer_signature_id),
                    )
                    .collect(),
                v.revision.structural_claim.len()
                    + v.revision.applicability_scope.description.len()
                    + v.revision.boundary_definition.len(),
            ),
            Self::Episode(v) => (
                v.basis.iter().collect(),
                v.members.iter().map(|m| m.reference.clone()).collect(),
                v.revision.producer_signature_id.into_iter().collect(),
                v.revision.boundary_explanation.len(),
            ),
            Self::Journal(v) => (
                v.points.iter().flat_map(|p| &p.basis).collect(),
                v.sources.clone(),
                v.revision.producer_signature_id.into_iter().collect(),
                v.revision.narrative.len() + v.points.iter().map(|p| p.text.len()).sum::<usize>(),
            ),
        };
        let mut sources = CognitionTraceSources {
            references: refs,
            producers,
            text_bytes: bytes,
        };
        sources.add_basis(basis);
        sources
    }
}
impl MemoryService {
    /// Read only this owner's exact nodes in the caller's captured view.
    pub async fn trace_nodes_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<Vec<CognitionContent>> {
        if references.len() > 128
            || references.iter().any(|reference| {
                reference.domain() != CognitiveDomain::Memory || !reference.is_cognition_revision()
            })
        {
            return Err(Error::Invalid(
                "Memory trace batch requires at most128 owned exact revisions".into(),
            ));
        }
        let policy =
            resolve_accessibility_policy(&self.configuration.snapshot_for_subject(subject)?)?;
        let ids = |kind| {
            references
                .iter()
                .filter(|r| r.kind() == kind)
                .filter_map(CognitiveRef::uuid)
                .collect::<Vec<_>>()
        };
        let mut contents = Vec::new();
        let memories = ids("memory_revision");
        if !memories.is_empty() {
            contents.extend(
                self.memory_views_in(
                    tx,
                    subject,
                    (&memories, &[]),
                    &policy,
                    None,
                    self.cognition.recorded_now(),
                    None,
                )
                .await?
                .into_values()
                .map(CognitionContent::Memory),
            );
        }
        let schemas = ids("cognitive_schema_revision");
        if !schemas.is_empty() {
            contents.extend(
                self.schema_views_in(tx, subject, &schemas, &[])
                    .await?
                    .into_values()
                    .map(CognitionContent::Schema),
            );
        }
        let episodes = ids("episode_revision");
        if !episodes.is_empty() {
            contents.extend(
                self.episode_views_in(tx, subject, &episodes, &[])
                    .await?
                    .into_values()
                    .map(CognitionContent::Episode),
            );
        }
        let journals = ids("journal_revision");
        if !journals.is_empty() {
            contents.extend(
                self.journal_views_in(tx, subject, &journals, &[])
                    .await?
                    .into_values()
                    .map(CognitionContent::Journal),
            );
        }
        if contents.len() != references.len() {
            return Err(Error::NotFound(
                "trace dependency was not found in Subject".into(),
            ));
        }
        contents.sort_by_key(|node| reference_parts(&node.reference()));
        Ok(contents)
    }
}

impl MemoryService {
    /// Current exact Memory anchors already supplied by the trusted invocation.
    /// Filter heads/lifecycle and byte costs before materializing complete bodies.
    pub async fn reflection_candidates(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        max_items: usize,
        max_bytes: usize,
    ) -> Result<(Vec<MemoryView>, bool)> {
        if references.len() > 1024
            || max_items == 0
            || max_items > 128
            || max_bytes == 0
            || max_bytes > 1048576
        {
            return Err(Error::Invalid(
                "invalid reflection Memory candidate budget".into(),
            ));
        }
        let ids = references
            .iter()
            .filter_map(|r| {
                if let CognitiveRef::MemoryRevision(id) = r {
                    Some(id.0)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let mut tx = self.store.read_view().await?;
        let rows=sqlx::query("SELECT r.memory_revision_id,2*octet_length(r.representation_text)+4096+COALESCE((SELECT sum(2*octet_length(to_jsonb(e)::text)) FROM memory_revision_evidence e WHERE e.memory_revision_id=r.memory_revision_id),0)+COALESCE((SELECT sum(2*octet_length(to_jsonb(d)::text)) FROM memory_revision_dependencies d WHERE d.memory_revision_id=r.memory_revision_id),0) AS read_bytes FROM memory_objects o JOIN memory_revisions r ON r.memory_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND r.memory_revision_id=ANY($2::uuid[]) AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' ORDER BY r.memory_revision_id LIMIT $3").bind(subject.0).bind(ids).bind((max_items+1) as i64).fetch_all(&mut *tx).await.map_err(db)?;
        let mut selected = Vec::new();
        let mut remaining = max_bytes;
        let mut omitted = false;
        for row in rows {
            let cost: i64 = row.try_get("read_bytes").map_err(db)?;
            if selected.len() >= max_items || cost as usize > remaining {
                omitted = true;
                continue;
            }
            remaining -= cost as usize;
            selected.push(CognitiveRef::MemoryRevision(MemoryRevisionId(
                row.try_get("memory_revision_id").map_err(db)?,
            )));
        }
        let result = self
            .trace_nodes_in(&mut tx, subject, &selected)
            .await?
            .into_iter()
            .filter_map(|c| {
                if let CognitionContent::Memory(m) = c {
                    Some(m)
                } else {
                    None
                }
            })
            .collect();
        tx.commit().await.map_err(db)?;
        Ok((result, omitted))
    }
}

impl MemoryService {
    /// Resolve lineage from formal dependency/evidence metadata before any narrative is read.
    pub async fn source_references_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<Vec<CognitiveRef>> {
        let id = reference
            .uuid()
            .ok_or_else(|| Error::Invalid("exact Memory reference required".into()))?;
        let statement = match reference {
            CognitiveRef::MemoryRevision(_) => {
                "SELECT 'occurrence'::text, e.occurrence_id::text FROM memory_revision_evidence e JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2 UNION SELECT CASE WHEN source_region_id IS NOT NULL THEN 'source_region' WHEN derived_representation_id IS NOT NULL THEN 'derived_representation' ELSE 'derived_region' END,COALESCE(source_region_id,derived_representation_id,derived_region_id)::text FROM memory_revision_evidence e JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2 AND num_nonnulls(source_region_id,derived_representation_id,derived_region_id)>0 UNION SELECT d.target_ref_kind,d.target_ref FROM memory_revision_dependencies d JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2"
            }
            CognitiveRef::CognitiveSchemaRevision(_) => {
                "SELECT CASE WHEN e.basis_kind='evidence' THEN 'occurrence' ELSE e.basis_kind END,CASE WHEN e.basis_kind='evidence' THEN e.occurrence_id::text ELSE e.basis_ref END FROM cognitive_schema_evidence_links e WHERE e.subject_id=$1 AND e.schema_revision_id=$2 AND e.revoked_at IS NULL UNION SELECT CASE WHEN source_region_id IS NOT NULL THEN 'source_region' WHEN derived_representation_id IS NOT NULL THEN 'derived_representation' ELSE 'derived_region' END,COALESCE(source_region_id,derived_representation_id,derived_region_id)::text FROM cognitive_schema_evidence_links e WHERE e.subject_id=$1 AND e.schema_revision_id=$2 AND e.revoked_at IS NULL AND num_nonnulls(source_region_id,derived_representation_id,derived_region_id)>0"
            }
            CognitiveRef::EpisodeRevision(_) => {
                "SELECT m.ref_kind,m.ref_value FROM episode_revision_members m JOIN episode_revisions r USING(episode_revision_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2 UNION SELECT CASE WHEN b.basis_kind='evidence' THEN 'occurrence' ELSE b.basis_kind END,CASE WHEN b.basis_kind='evidence' THEN b.occurrence_id::text ELSE b.basis_ref END FROM episode_revision_basis b JOIN episode_revisions r USING(episode_revision_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2 UNION SELECT CASE WHEN source_region_id IS NOT NULL THEN 'source_region' WHEN derived_representation_id IS NOT NULL THEN 'derived_representation' ELSE 'derived_region' END,COALESCE(source_region_id,derived_representation_id,derived_region_id)::text FROM episode_revision_basis b JOIN episode_revisions r USING(episode_revision_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2 AND num_nonnulls(source_region_id,derived_representation_id,derived_region_id)>0"
            }
            CognitiveRef::JournalRevision(_) => {
                "SELECT s.ref_kind,s.ref_value FROM journal_revision_sources s JOIN journal_revisions r USING(journal_revision_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2"
            }
            _ => {
                return Err(Error::Invalid(
                    "Memory source metadata requires exact owned revision".into(),
                ));
            }
        };
        let rows: Vec<(String, String)> = sqlx::query_as(statement)
            .bind(subject.0)
            .bind(id)
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?;
        let mut refs = rows
            .into_iter()
            .map(|(k, v)| parse_reference(&k, &v))
            .collect::<Result<Vec<_>>>()?;
        if matches!(reference, CognitiveRef::JournalRevision(_)) {
            let bases:Vec<serde_json::Value>=sqlx::query_scalar("SELECT p.basis FROM journal_point_basis p JOIN journal_revisions r USING(journal_revision_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2").bind(subject.0).bind(id).fetch_all(&mut **tx).await.map_err(db)?;
            for value in bases {
                let basis: RevisionBasis = serde_json::from_value(value)
                    .map_err(|_| Error::Unavailable("invalid Journal source metadata".into()))?;
                refs.push(basis.evidence_handle().reference);
                if let RevisionBasis::Evidence(e) = basis {
                    refs.push(CognitiveRef::Occurrence(e.occurrence_id));
                }
            }
        }
        Ok(refs)
    }
}
