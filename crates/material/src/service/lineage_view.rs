//! A bounded, coherent lineage read; clients never traverse the storage graph.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_persistence::database_error as db;
use std::collections::HashSet;

pub enum MaterialLineageNode {
    Occurrence(ObservationOccurrence),
    Artifact(Artifact),
    SourceRegion(SourceRegion),
    Representation(Box<DerivedRepresentation>),
    DerivedRegion(DerivedRegion),
}
impl MaterialLineageNode {
    pub fn reference(&self) -> CognitiveRef {
        match self {
            Self::Occurrence(v) => CognitiveRef::Occurrence(v.occurrence_id),
            Self::Artifact(v) => CognitiveRef::Artifact(v.artifact_id),
            Self::SourceRegion(v) => CognitiveRef::SourceRegion(v.source_region_id),
            Self::Representation(v) => {
                CognitiveRef::DerivedRepresentation(v.derived_representation_id)
            }
            Self::DerivedRegion(v) => CognitiveRef::DerivedRegion(v.derived_region_id),
        }
    }
    fn limit_payload(&mut self, remaining: &mut usize) -> Result<bool> {
        let (text, payload) = match self {
            Self::Representation(v) => (&mut v.payload_text, Some(&mut v.payload_json)),
            _ => return Ok(false),
        };
        let mut truncated = false;
        if let Some(text) = text {
            let end = text.floor_char_boundary((*remaining).min(text.len()));
            truncated = end < text.len();
            text.truncate(end);
            *remaining -= end;
        }
        let Some(payload) = payload else {
            return Ok(truncated);
        };
        let Some(value) = payload.as_ref() else {
            return Ok(truncated);
        };
        let bytes = serde_json::to_vec(value)
            .map_err(|e| Error::Infrastructure(e.to_string()))?
            .len();
        if bytes > *remaining {
            *payload = None;
            truncated = true;
        } else {
            *remaining -= bytes;
        }
        Ok(truncated)
    }
}
pub struct MaterialLineage {
    pub nodes: Vec<MaterialLineageNode>,
    pub truncated: bool,
    pub payloads_truncated: bool,
}
impl MaterialService {
    pub async fn lineage_view(
        &self,
        subject: SubjectId,
        roots: &[CognitiveRef],
        max_nodes: usize,
        max_bytes: usize,
    ) -> Result<MaterialLineage> {
        if roots.is_empty()
            || roots.len() > 512
            || !(1..=512).contains(&max_nodes)
            || !(1..=1_048_576).contains(&max_bytes)
        {
            return Err(Error::Invalid(
                "lineage requires 1..512 roots, 1..512 nodes and 1..1048576 payload bytes".into(),
            ));
        }
        let mut visited = HashSet::new();
        let mut pending = roots.to_vec();
        let mut nodes = Vec::new();
        let mut tx = self.store.read_view().await?;
        while !pending.is_empty() && nodes.len() < max_nodes {
            let mut batch = Vec::new();
            let remaining = max_nodes - nodes.len();
            while let Some(reference) = pending.pop() {
                if !matches!(
                    reference,
                    CognitiveRef::Occurrence(_)
                        | CognitiveRef::Artifact(_)
                        | CognitiveRef::SourceRegion(_)
                        | CognitiveRef::DerivedRepresentation(_)
                        | CognitiveRef::DerivedRegion(_)
                ) {
                    return Err(Error::Invalid(
                        "lineage roots must be Material references".into(),
                    ));
                }
                if visited.insert(reference.clone()) {
                    batch.push(reference);
                }
                if batch.len() == remaining {
                    break;
                }
            }
            let values = self.lineage_nodes_in(&mut tx, subject, &batch).await?;
            for node in &values {
                match node {
                    MaterialLineageNode::Occurrence(value) => {
                        pending.extend(value.artifact_id.map(CognitiveRef::Artifact))
                    }
                    MaterialLineageNode::Artifact(_) => {}
                    MaterialLineageNode::SourceRegion(value) => {
                        pending.push(CognitiveRef::Artifact(value.artifact_id));
                        pending.extend(
                            value
                                .parent_source_region_id
                                .map(CognitiveRef::SourceRegion),
                        );
                    }
                    MaterialLineageNode::Representation(value) => {
                        pending.extend(value.inputs.iter().map(|input| input.reference.clone()))
                    }
                    MaterialLineageNode::DerivedRegion(value) => {
                        pending.push(CognitiveRef::DerivedRepresentation(
                            value.derived_representation_id,
                        ));
                        pending.extend(
                            value
                                .parent_derived_region_id
                                .map(CognitiveRef::DerivedRegion),
                        );
                    }
                }
            }
            nodes.extend(values);
            pending.retain(|reference| !visited.contains(reference));
        }
        let truncated = !pending.is_empty();
        tx.commit().await.map_err(db)?;
        nodes.sort_by_key(|node| node.reference().to_string());
        let mut remaining = max_bytes;
        let mut payloads_truncated = false;
        for node in &mut nodes {
            payloads_truncated |= node.limit_payload(&mut remaining)?;
        }
        Ok(MaterialLineage {
            nodes,
            truncated,
            payloads_truncated,
        })
    }
}
