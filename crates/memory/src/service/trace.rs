//! Cognition owns its dependency graph; Material owns the evidence lineage.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::HashSet;

pub enum CognitionContent {
    Memory(MemoryView),
    Schema(super::schema::SchemaView),
    Episode(EpisodeView),
    Journal(JournalView),
}
impl CognitionContent {
    fn reference(&self) -> CognitiveRef {
        match self {
            Self::Memory(v) => CognitiveRef::MemoryRevision(v.revision.memory_revision_id),
            Self::Schema(v) => CognitiveRef::CognitiveSchemaRevision(v.revision.schema_revision_id),
            Self::Episode(v) => CognitiveRef::EpisodeRevision(v.revision.episode_revision_id),
            Self::Journal(v) => CognitiveRef::JournalRevision(v.revision.journal_revision_id),
        }
    }
}

pub struct CognitionTrace {
    pub nodes: Vec<CognitionContent>,
    pub material_roots: Vec<CognitiveRef>,
    pub producers: Vec<Uuid>,
    pub text_bytes: usize,
    pub truncated: bool,
}
impl CognitionTrace {
    fn admit_sources(
        &mut self,
        refs: Vec<CognitiveRef>,
        pending: &mut Vec<CognitiveRef>,
        max_nodes: usize,
    ) {
        for reference in refs {
            if is_revision(&reference) {
                pending.push(reference);
                continue;
            }
            if !matches!(
                reference,
                CognitiveRef::Occurrence(_)
                    | CognitiveRef::Artifact(_)
                    | CognitiveRef::SourceRegion(_)
                    | CognitiveRef::DerivedRegion(_)
                    | CognitiveRef::DerivedRepresentation(_)
            ) || self.material_roots.contains(&reference)
            {
                continue;
            }
            if self.material_roots.len() == max_nodes {
                self.truncated = true;
                continue;
            }
            self.material_roots.push(reference);
        }
    }
}
impl CognitionContent {
    fn trace_support(&self) -> (Vec<CognitiveRef>, Vec<Uuid>, usize) {
        let (basis, mut refs, mut producers, bytes): (Vec<_>, Vec<_>, Vec<_>, _) = match self {
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
        for support in basis {
            match support {
                RevisionBasis::CognitionDependency(v) => refs.push(v.target_revision.clone()),
                RevisionBasis::Evidence(v) => {
                    refs.push(CognitiveRef::Occurrence(v.occurrence_id));
                    refs.push(v.cognitive_ref());
                }
                RevisionBasis::Seed(_) => {}
            }
        }
        producers.sort_unstable();
        producers.dedup();
        (refs, producers, bytes)
    }
}
impl MemoryService {
    pub async fn trace(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
        max_nodes: usize,
        max_bytes: usize,
    ) -> Result<CognitionTrace> {
        if !is_revision(&reference)
            || !(1..=512).contains(&max_nodes)
            || !(1..=1_048_576).contains(&max_bytes)
        {
            return Err(Error::Invalid("trace requires an exact cognition revision, 1..512 nodes and 1..1048576 text bytes".into()));
        }
        let policy =
            resolve_accessibility_policy(&self.configuration.snapshot_for_subject(subject)?)?;
        let mut tx = self.store.read_view().await?;
        let mut trace = CognitionTrace {
            nodes: vec![],
            material_roots: vec![],
            producers: vec![],
            text_bytes: 0,
            truncated: false,
        };
        let mut pending = vec![reference];
        let mut visited = HashSet::new();
        while !pending.is_empty() {
            pending.retain(|r| !visited.contains(r));
            pending.sort_by_key(reference_parts);
            pending.dedup();
            let remaining = max_nodes - trace.nodes.len();
            if pending.len() > remaining {
                trace.truncated = true;
                pending.truncate(remaining);
            }
            if pending.is_empty() {
                break;
            }
            let batch = pending.drain(..pending.len().min(128)).collect::<Vec<_>>();
            let ids = |kind| {
                batch
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
                        &mut tx,
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
                    self.schema_views_in(&mut tx, subject, &schemas, &[])
                        .await?
                        .into_values()
                        .map(CognitionContent::Schema),
                );
            }
            let episodes = ids("episode_revision");
            if !episodes.is_empty() {
                contents.extend(
                    self.episode_views_in(&mut tx, subject, &episodes, &[])
                        .await?
                        .into_values()
                        .map(CognitionContent::Episode),
                );
            }
            let journals = ids("journal_revision");
            if !journals.is_empty() {
                contents.extend(
                    self.journal_views_in(&mut tx, subject, &journals, &[])
                        .await?
                        .into_values()
                        .map(CognitionContent::Journal),
                );
            }
            if contents.len() != batch.len() {
                return Err(Error::NotFound(
                    "trace dependency was not found in Subject".into(),
                ));
            }
            contents.sort_by_key(|v| reference_parts(&v.reference()));
            for content in contents {
                let (refs, producers, bytes) = content.trace_support();
                if bytes > max_bytes - trace.text_bytes && trace.nodes.is_empty() {
                    return Err(Error::Invalid("trace text budget is smaller than the requested revision; increase max_bytes".into()));
                }
                if bytes > max_bytes - trace.text_bytes {
                    trace.truncated = true;
                    continue;
                }
                visited.insert(content.reference());
                trace.text_bytes += bytes;
                trace.producers.extend(producers);
                trace.admit_sources(refs, &mut pending, max_nodes);
                trace.nodes.push(content);
            }
        }
        tx.commit().await.map_err(db)?;
        trace.producers.sort_unstable();
        trace.producers.dedup();
        Ok(trace)
    }
}

fn is_revision(reference: &CognitiveRef) -> bool {
    matches!(
        reference,
        CognitiveRef::MemoryRevision(_)
            | CognitiveRef::CognitiveSchemaRevision(_)
            | CognitiveRef::EpisodeRevision(_)
            | CognitiveRef::JournalRevision(_)
    )
}
