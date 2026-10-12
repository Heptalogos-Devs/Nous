// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{assets::lifecycle::OpenArtifact, *};
use arc_swap::ArcSwap;

#[derive(Clone, Default)]
pub struct ServingSnapshot {
    pub view_digest: Option<String>,
    pub lexical: Option<Arc<LexicalGeneration>>,
    pub dense: Vec<Arc<DenseGeneration>>,
    pub concept: Vec<Arc<ConceptGeneration>>,
    pub topology: Option<Arc<WaveGraphGeneration>>,
    pub vcp: Option<Arc<VcpServingGeneration>>,
}

#[derive(Clone)]
pub(crate) struct ServingPublisher {
    current: Arc<ArcSwap<BTreeMap<nous_core::SubjectId, Arc<ServingSnapshot>>>>,
}

impl Default for ServingPublisher {
    fn default() -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(BTreeMap::new())),
        }
    }
}

impl ServingPublisher {
    pub(crate) fn update_for(
        &self,
        subject: nous_core::SubjectId,
        update: impl Fn(&mut ServingSnapshot),
    ) {
        self.current.rcu(|current| {
            let mut next = current.as_ref().clone();
            let mut snapshot = next
                .get(&subject)
                .map(|snapshot| snapshot.as_ref().clone())
                .unwrap_or_default();
            update(&mut snapshot);
            next.insert(subject, Arc::new(snapshot));
            Arc::new(next)
        });
    }
    pub(crate) fn snapshot_for(&self, subject: nous_core::SubjectId) -> Arc<ServingSnapshot> {
        self.current
            .load()
            .get(&subject)
            .cloned()
            .unwrap_or_else(|| Arc::new(ServingSnapshot::default()))
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
        }
    }
    pub(crate) fn artifact(&self, id: ServingGenerationId) -> Option<OpenArtifact> {
        if let Some(value) = self
            .lexical
            .as_ref()
            .filter(|value| value.generation_id == id)
        {
            return Some(OpenArtifact::Lexical(value.clone()));
        }
        if let Some(value) = self.dense.iter().find(|value| value.generation_id == id) {
            return Some(OpenArtifact::Dense(value.clone()));
        }
        if let Some(value) = self.concept.iter().find(|value| value.generation_id == id) {
            return Some(OpenArtifact::Concept(value.clone()));
        }
        if let Some(value) = self
            .topology
            .as_ref()
            .filter(|value| value.generation_id == id)
        {
            return Some(OpenArtifact::Topology(value.clone()));
        }
        self.vcp
            .as_ref()
            .filter(|value| value.generation_id == id)
            .map(|value| OpenArtifact::Vcp(value.clone()))
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
            OpenArtifact::Dense(index) => {
                self.dense
                    .retain(|old| old.space.space_hash != index.space.space_hash);
                self.dense.push(index);
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
