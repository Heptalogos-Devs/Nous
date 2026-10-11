// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{assets::lifecycle::OpenArtifact, *};
use arc_swap::ArcSwap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Default)]
pub struct ServingSnapshot {
    pub generation: u64,
    pub view_digest: Option<String>,
    pub lexical: Option<Arc<LexicalGeneration>>,
    pub dense: Vec<Arc<DenseGeneration>>,
    pub concept: Vec<Arc<ConceptGeneration>>,
    pub topology: Option<Arc<WaveGraphGeneration>>,
    pub vcp: Option<Arc<VcpServingGeneration>>,
    pub epa: Vec<Arc<EpaBasisGeneration>>,
}

#[derive(Clone)]
pub struct ServingPublisher {
    current: Arc<ArcSwap<BTreeMap<nous_core::SubjectId, Arc<ServingSnapshot>>>>,
    next_generation: Arc<AtomicU64>,
}

impl Default for ServingPublisher {
    fn default() -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(BTreeMap::new())),
            next_generation: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl ServingPublisher {
    pub fn update_for(&self, subject: nous_core::SubjectId, update: impl Fn(&mut ServingSnapshot)) {
        let generation = self.next_generation();
        self.current.rcu(|current| {
            let mut next = current.as_ref().clone();
            let mut snapshot = next
                .get(&subject)
                .map(|snapshot| snapshot.as_ref().clone())
                .unwrap_or_default();
            update(&mut snapshot);
            snapshot.generation = generation;
            next.insert(subject, Arc::new(snapshot));
            Arc::new(next)
        });
    }
    /// Allocate a process-local monotonic generation number.  The database
    /// generation row remains the durable identity; this number is only the
    /// in-process publication trace.
    pub fn next_generation(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::Relaxed)
    }

    pub fn snapshot_for(&self, subject: nous_core::SubjectId) -> Arc<ServingSnapshot> {
        self.current
            .load()
            .get(&subject)
            .cloned()
            .unwrap_or_else(|| Arc::new(ServingSnapshot::default()))
    }

    pub fn snapshot(&self) -> Arc<ServingSnapshot> {
        self.current
            .load()
            .values()
            .max_by_key(|snapshot| snapshot.generation)
            .cloned()
            .unwrap_or_else(|| Arc::new(ServingSnapshot::default()))
    }

    pub fn publish_for(&self, subject: nous_core::SubjectId, snapshot: ServingSnapshot) {
        let snapshot = Arc::new(snapshot);
        self.current.rcu(|current| {
            let mut next = current.as_ref().clone();
            next.insert(subject, snapshot.clone());
            Arc::new(next)
        });
    }
}

impl ServingSnapshot {
    pub fn generation_trace(&self) -> QueryGenerationTrace {
        QueryGenerationTrace {
            lexical: self.lexical.as_ref().map(|g| g.generation_id),
            dense: self.dense.iter().map(|g| g.generation_id).collect(),
            topology: self
                .topology
                .as_ref()
                .map(|g| g.generation_id)
                .or_else(|| self.vcp.as_ref().map(|g| g.generation_id)),
            epa_basis: self.epa.first().map(|g| g.generation_id),
        }
    }
    pub(crate) fn retain_generations(
        &mut self,
        ids: impl Iterator<Item = nous_core::ServingGenerationId>,
    ) {
        let ids: std::collections::HashSet<_> = ids.collect();
        self.lexical = self
            .lexical
            .take()
            .filter(|value| ids.contains(&value.generation_id));
        self.dense
            .retain(|value| ids.contains(&value.generation_id));
        self.concept
            .retain(|value| ids.contains(&value.generation_id));
        self.epa.retain(|value| ids.contains(&value.generation_id));
        self.topology = self
            .topology
            .take()
            .filter(|value| ids.contains(&value.generation_id));
        self.vcp = self
            .vcp
            .take()
            .filter(|value| ids.contains(&value.generation_id));
    }
    pub(crate) fn contains_generation(&self, id: nous_core::ServingGenerationId) -> bool {
        self.lexical
            .as_ref()
            .is_some_and(|value| value.generation_id == id)
            || self.dense.iter().any(|value| value.generation_id == id)
            || self.concept.iter().any(|value| value.generation_id == id)
            || self
                .topology
                .as_ref()
                .is_some_and(|value| value.generation_id == id)
            || self
                .vcp
                .as_ref()
                .is_some_and(|value| value.generation_id == id)
    }
}

impl ServingSnapshot {
    pub(crate) fn install(&mut self, artifact: OpenArtifact) {
        match artifact {
            OpenArtifact::Lexical(index) => self.lexical = Some(index),
            OpenArtifact::Dense(index, basis) => {
                self.dense
                    .retain(|old| old.space.space_hash != index.space.space_hash);
                self.epa
                    .retain(|old| old.basis.embedding_space != index.space.space_hash);
                self.dense.push(index);
                self.epa.extend(basis);
            }
            OpenArtifact::Topology(graph) => {
                self.topology = Some(graph);
                self.vcp = None;
            }
            OpenArtifact::Vcp(assets) => {
                self.vcp = Some(assets);
                self.topology = None;
            }
            OpenArtifact::Concept(generation) => {
                self.concept.retain(|old| {
                    old.space.as_ref().map(|s| &s.space_hash)
                        != generation.space.as_ref().map(|s| &s.space_hash)
                });
                self.concept.push(generation);
            }
        }
    }
}
