// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_core::{CognitionTraceSources, CognitiveDomain, CognitiveRef, Result};
use std::collections::HashSet;

fn memory_node(node: nous_memory::CognitionContent) -> p::CognitionTraceNode {
    use nous_memory::CognitionContent as N;
    use p::cognition_trace_node::Content as C;
    let reference = node.reference();
    let content = match node {
        N::Memory(value) => C::Memory(super::memory::view(value)),
        N::Schema(value) => C::Schema(super::schema::schema_view(value)),
        N::Episode(value) => C::Episode(super::episode::revision(
            value.revision,
            value.members,
            value.basis,
            value.relations,
        )),
        N::Journal(value) => C::Journal(super::journal::journal_revision_proto(value)),
    };
    p::CognitionTraceNode {
        reference: Some(to_ref(reference)),
        content: Some(content),
    }
}
impl KernelService {
    /// Compose owner reads in one captured cognition view. Material retains its
    /// existing bounded lineage operation; no owner recursively impersonates another.
    pub(super) async fn trace(
        &self,
        request: p::CognitionTraceRequest,
    ) -> Result<p::CognitionTrace> {
        let subject = SubjectId(id(&request.subject_id)?);
        let root = from_ref(required(request.reference, "reference")?)?;
        let max_nodes = if request.max_nodes == 0 {
            64
        } else {
            request.max_nodes as usize
        };
        let max_bytes = if request.max_bytes == 0 {
            65536
        } else {
            request.max_bytes as usize
        };
        if !root.is_cognition_revision()
            || !(1..=512).contains(&max_nodes)
            || !(1..=1_048_576).contains(&max_bytes)
        {
            return Err(Error::Invalid("Trace requires an exact cognition revision, 1..512 nodes and 1..1048576 text bytes".into()));
        }
        let mut tx = self.0.store.read_view().await?;
        let mut pending = vec![root.clone()];
        let mut visited = HashSet::new();
        let mut nodes = Vec::new();
        let mut material = Vec::new();
        let mut producers = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        while !pending.is_empty() {
            pending.retain(|reference| !visited.contains(reference));
            pending.sort_by_key(|reference| (reference.kind(), reference.uuid()));
            pending.dedup();
            let remaining = max_nodes - nodes.len();
            if pending.len() > remaining {
                pending.truncate(remaining);
                truncated = true;
            }
            if pending.is_empty() {
                break;
            }
            let batch = pending.drain(..pending.len().min(128)).collect::<Vec<_>>();
            let mut read = self.read_trace_nodes_in(&mut tx, subject, &batch).await?;
            read.sort_by_key(|(node, _)| {
                node.reference
                    .as_ref()
                    .map(|reference| (reference.kind.clone(), reference.value.clone()))
            });
            for (node, sources) in read {
                let reference = from_ref(node.reference.clone().expect("owner node reference"))?;
                visited.insert(reference);
                if nodes.is_empty() && sources.text_bytes > max_bytes - bytes {
                    return Err(Error::Invalid(
                        "Trace budget is smaller than requested revision; increase max_bytes"
                            .into(),
                    ));
                }
                if sources.text_bytes > max_bytes - bytes {
                    truncated = true;
                    continue;
                }
                bytes += sources.text_bytes;
                producers.extend(sources.producers);
                truncated |=
                    admit_sources(sources.references, &mut pending, &mut material, max_nodes);
                nodes.push(node);
            }
        }
        tx.commit()
            .await
            .map_err(nous_persistence::database_error)?;
        producers.sort_unstable();
        producers.dedup();
        let remaining_nodes = max_nodes - nodes.len();
        let remaining_bytes = max_bytes - bytes;
        let has_material = !material.is_empty();
        let lineage = if has_material && remaining_nodes > 0 && remaining_bytes > 0 {
            Some(
                self.read_lineage(p::MaterialLineageRequest {
                    subject_id: subject.0.to_string(),
                    roots: material.into_iter().map(to_ref).collect(),
                    max_nodes: remaining_nodes as u32,
                    max_bytes: remaining_bytes as u32,
                })
                .await?,
            )
        } else {
            None
        };
        truncated |= (has_material && lineage.is_none())
            || lineage
                .as_ref()
                .is_some_and(|lineage| lineage.truncated || lineage.payloads_truncated);
        let producers = self
            .0
            .material
            .producer_signatures(&producers)
            .await?
            .into_iter()
            .map(evidence::producer)
            .collect();
        Ok(p::CognitionTrace {
            reference: Some(to_ref(root)),
            nodes,
            lineage,
            producers,
            truncated,
        })
    }
    async fn read_trace_nodes_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<Vec<(p::CognitionTraceNode, CognitionTraceSources)>> {
        let memory = references
            .iter()
            .filter(|reference| reference.domain() == CognitiveDomain::Memory)
            .cloned()
            .collect::<Vec<_>>();
        let mut read = Vec::<(p::CognitionTraceNode, CognitionTraceSources)>::new();
        if !memory.is_empty() {
            for node in self
                .0
                .require_memory()?
                .trace_nodes_in(tx, subject, &memory)
                .await?
            {
                let sources = node.trace_sources();
                read.push((memory_node(node), sources));
            }
        }
        for reference in references
            .iter()
            .filter(|reference| reference.domain() != CognitiveDomain::Memory)
        {
            if reference.domain() == CognitiveDomain::SelfCognition {
                let (node, sources) = self
                    .0
                    .require_self()?
                    .trace_node_in(tx, subject, reference)
                    .await?;
                read.push((
                    p::CognitionTraceNode {
                        reference: Some(to_ref(reference.clone())),
                        content: Some(p::cognition_trace_node::Content::SelfRecord(
                            super::self_cognition::record(node),
                        )),
                    },
                    sources,
                ));
            } else if reference.domain() == CognitiveDomain::SocialCognition {
                let (node, sources) = self
                    .0
                    .require_social()?
                    .trace_node_in(tx, subject, reference)
                    .await?;
                read.push((
                    p::CognitionTraceNode {
                        reference: Some(to_ref(reference.clone())),
                        content: Some(p::cognition_trace_node::Content::SocialRecord(
                            super::social_cognition::record(node),
                        )),
                    },
                    sources,
                ));
            } else if reference.domain() == CognitiveDomain::Motivation {
                let (node, sources) = self
                    .0
                    .require_motivation()?
                    .trace_node_in(tx, subject, reference)
                    .await?;
                read.push((
                    p::CognitionTraceNode {
                        reference: Some(to_ref(reference.clone())),
                        content: Some(p::cognition_trace_node::Content::MotivationRecord(
                            super::motivation::record(node),
                        )),
                    },
                    sources,
                ));
            } else if let CognitiveRef::CognitiveSeedVersion(version) = reference {
                let seed = self
                    .0
                    .subjects
                    .cognitive_seed_version_in(tx, subject, *version)
                    .await?;
                let sources = CognitionTraceSources {
                    text_bytes: seed.text.len(),
                    references: vec![CognitiveRef::Artifact(seed.version.artifact_id)],
                    ..Default::default()
                };
                read.push((
                    p::CognitionTraceNode {
                        reference: Some(to_ref(reference.clone())),
                        content: Some(p::cognition_trace_node::Content::Seed(
                            super::subject::seed_source(seed),
                        )),
                    },
                    sources,
                ));
            } else {
                return Err(Error::Invalid("Unsupported trace source".into()));
            }
        }
        Ok(read)
    }
}

fn admit_sources(
    references: Vec<CognitiveRef>,
    pending: &mut Vec<CognitiveRef>,
    material: &mut Vec<CognitiveRef>,
    max_nodes: usize,
) -> bool {
    let mut truncated = false;
    for source in references {
        if source.is_cognition_revision() || matches!(source, CognitiveRef::CognitiveSeedVersion(_))
        {
            pending.push(source);
        } else if source.domain() == CognitiveDomain::Material && !material.contains(&source) {
            if material.len() == max_nodes {
                truncated = true;
            } else {
                material.push(source);
            }
        }
    }
    truncated
}
