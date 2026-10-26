//! Content-free source facts survive permanent erasure of their owned payload.
use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurgedMemory {
    pub id: MemoryId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub revision: i64,
    pub commit_seq: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceTombstoneOrigin {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub target: RetrievalTarget,
    pub recorded_at: String,
    pub commit_seq: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceTombstone {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub target: RetrievalTarget,
    pub sources: Vec<EvidenceSource>,
    pub recorded_at: String,
    pub commit_seq: i64,
    pub origin: Option<SourceTombstoneOrigin>,
}
