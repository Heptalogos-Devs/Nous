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
                    self.cognition.now(subject),
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
