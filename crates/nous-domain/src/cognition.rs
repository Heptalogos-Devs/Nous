//! Memory authority and its grounded formation inputs and results.
use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDraft {
    pub title: String,
    pub content: String,
    pub epistemic_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: MemoryId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub revision: i64,
    pub title: String,
    pub content: String,
    pub epistemic_status: String,
    pub observation_id: ObservationId,
    pub material_id: MaterialId,
    pub model_call_id: Option<ModelCallId>,
    #[serde(default)]
    pub origin: MemoryOrigin,
    pub times: CognitiveTimes,
    pub recorded_at: String,
    pub deleted: bool,
    #[serde(default)]
    pub lifecycle: MemoryLifecycle,
    pub commit_seq: i64,
}

impl Memory {
    pub fn is_active(&self) -> bool {
        !self.deleted && self.lifecycle.activation == MemoryActivation::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormationRun {
    pub job: JobRecord,
    pub observation_id: ObservationId,
    pub material_id: MaterialId,
    pub subject_revision: i64,
    pub material_hash: String,
    pub memory_ids: Vec<MemoryId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySource {
    pub memory: Memory,
    pub observation: Observation,
    pub material: MaterialContent,
    pub model_call: Option<ModelCall>,
    #[serde(default)]
    pub embedding_provenance: Vec<EmbeddingProvenance>,
}
