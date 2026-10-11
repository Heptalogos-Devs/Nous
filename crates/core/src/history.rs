//! Rebuildable historical read descriptors; canonical Authority remains with semantic owners.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
struct HistoricalLookup {
    cognition: HashMap<CognitiveRef, usize>,
    selected_revisions: HashSet<CognitiveRef>,
    tags: HashMap<TagId, usize>,
    material_visibility: HashSet<CognitiveRef>,
    material_documents: HashSet<CognitiveRef>,
    associations: HashSet<AssociationEvidenceId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalCognitionState {
    pub object: CognitiveRef,
    pub head: CognitiveRef,
    pub revisions: Vec<CognitiveRef>,
    pub recorded_revisions: Vec<CognitiveRef>,
    pub state: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalTagState {
    pub tag: TagId,
    pub revision_id: Uuid,
    pub canonical_tag: Option<TagId>,
    pub status: String,
    pub semantic: TagSemanticRepresentation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalLexicalVisibility {
    pub lexical_ref: String,
    pub object_kind: String,
    pub canonical_ref: String,
    pub display_name: String,
    pub aliases: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalAuthoritySnapshot {
    pub subject: SubjectId,
    pub as_of: chrono::DateTime<chrono::Utc>,
    pub revision_view: RevisionView,
    pub cognition: Vec<HistoricalCognitionState>,
    pub tags: Vec<HistoricalTagState>,
    pub associations: Vec<AssociationEvidenceId>,
    pub schema_evidence_links: Vec<Uuid>,
    pub entity_bindings: Vec<Uuid>,
    pub material_visibility: Vec<CognitiveRef>,
    pub material_documents: Vec<CognitiveRef>,
    pub lexical_visibility: Vec<HistoricalLexicalVisibility>,
    pub snapshot_digest: String,
    /// Derived only from a captured view. It is never part of the durable snapshot.
    #[serde(skip)]
    lookup: OnceLock<HistoricalLookup>,
}
impl HistoricalAuthoritySnapshot {
    pub fn empty(
        subject: SubjectId,
        as_of: chrono::DateTime<chrono::Utc>,
        revision_view: RevisionView,
    ) -> Self {
        Self {
            subject,
            as_of,
            revision_view,
            cognition: vec![],
            tags: vec![],
            associations: vec![],
            schema_evidence_links: vec![],
            entity_bindings: vec![],
            material_visibility: vec![],
            material_documents: vec![],
            lexical_visibility: vec![],
            snapshot_digest: String::new(),
            lookup: OnceLock::new(),
        }
    }
    pub fn refresh_digest(&mut self) -> Result<()> {
        // Owners finish assembly before capture; any builder mutation refreshes both identities.
        self.lookup.take();
        let identities=self.cognition.iter().map(|state|serde_json::json!({"object":state.object,"head":state.head,"revisions":state.revisions,"acceptance":state.state["acceptance_state"],"integrity":state.state["integrity_state"],"suppression":state.state["suppression_state"],"purge":state.state["purge_state"],"accessibility":state.state["accessibility_mode"]})).collect::<Vec<_>>();
        let value = serde_json::json!({"subject":self.subject,"revision_view":self.revision_view,"cognition":identities,"tags":self.tags,"associations":self.associations,"schema_evidence_links":self.schema_evidence_links,"entity_bindings":self.entity_bindings,"material":self.material_visibility,"material_documents":self.material_documents,"lexical":self.lexical_visibility});
        self.snapshot_digest = blake3::hash(
            &serde_json::to_vec(&value)
                .map_err(|error| Error::Infrastructure(error.to_string()))?,
        )
        .to_hex()
        .to_string();
        Ok(())
    }
    pub fn canonical_tag(&self, requested: TagId) -> Option<TagId> {
        let mut current = requested;
        for _ in 0..128 {
            let tag = self.tag_state(current)?;
            match tag.status.as_str() {
                "active" => return Some(current),
                "merged" => current = tag.canonical_tag?,
                _ => return None,
            }
        }
        None
    }
    pub fn cognition_for(&self, reference: &CognitiveRef) -> Option<&HistoricalCognitionState> {
        self.index()
            .cognition
            .get(reference)
            .map(|index| &self.cognition[*index])
    }
    pub fn selected_cognition_for(
        &self,
        reference: &CognitiveRef,
    ) -> Option<&HistoricalCognitionState> {
        self.index()
            .selected_revisions
            .contains(reference)
            .then(|| self.cognition_for(reference))
            .flatten()
    }
    pub fn tag_state(&self, tag: TagId) -> Option<&HistoricalTagState> {
        self.index().tags.get(&tag).map(|index| &self.tags[*index])
    }
    pub fn material_contains(&self, reference: &CognitiveRef) -> bool {
        self.index().material_visibility.contains(reference)
    }
    pub fn material_document_contains(&self, reference: &CognitiveRef) -> bool {
        self.index().material_documents.contains(reference)
    }
    pub fn association_contains(&self, association: AssociationEvidenceId) -> bool {
        self.index().associations.contains(&association)
    }
    fn index(&self) -> &HistoricalLookup {
        self.lookup.get_or_init(|| {
            let mut lookup = HistoricalLookup::default();
            for (index, state) in self.cognition.iter().enumerate() {
                lookup.cognition.insert(state.object.clone(), index);
                lookup.cognition.extend(
                    state
                        .recorded_revisions
                        .iter()
                        .cloned()
                        .map(|reference| (reference, index)),
                );
                lookup
                    .selected_revisions
                    .extend(state.revisions.iter().cloned());
            }
            lookup.tags.extend(
                self.tags
                    .iter()
                    .enumerate()
                    .map(|(index, tag)| (tag.tag, index)),
            );
            lookup
                .material_visibility
                .extend(self.material_visibility.iter().cloned());
            lookup
                .material_documents
                .extend(self.material_documents.iter().cloned());
            lookup
                .associations
                .extend(self.associations.iter().copied());
            lookup
        })
    }
    pub fn contains(&self, reference: &CognitiveRef) -> bool {
        self.cognition_for(reference).is_some()
            || self.material_contains(reference)
            || matches!(reference,CognitiveRef::Tag(tag) if self.canonical_tag(*tag).is_some())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalMaterialProjection {
    pub lexical_visibility: Vec<HistoricalLexicalVisibility>,
    pub known_references: Vec<CognitiveRef>,
    pub document_references: Vec<CognitiveRef>,
    pub entity_bindings: Vec<Uuid>,
}
