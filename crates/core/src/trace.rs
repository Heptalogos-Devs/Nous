//! Owner-declared sources for the shared cognition trace operation.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{CognitiveRef, RevisionBasis};
use uuid::Uuid;

#[derive(Default)]
pub struct CognitionTraceSources {
    pub references: Vec<CognitiveRef>,
    pub producers: Vec<Uuid>,
    pub text_bytes: usize,
}
impl CognitionTraceSources {
    pub fn add_basis<'a>(&mut self, basis: impl IntoIterator<Item = &'a RevisionBasis>) {
        for entry in basis {
            match entry {
                RevisionBasis::Evidence(evidence) => {
                    self.references
                        .push(CognitiveRef::Occurrence(evidence.occurrence_id));
                    self.references.push(evidence.cognitive_ref());
                }
                RevisionBasis::CognitionDependency(source) => {
                    self.references.push(source.target_revision.clone())
                }
                RevisionBasis::Seed(seed) => self
                    .references
                    .push(CognitiveRef::CognitiveSeedVersion(seed.seed_version_id)),
            }
        }
    }
}
