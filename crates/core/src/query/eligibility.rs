// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::QueryConstraints;
use crate::{AuthorityClass, EntityRef, EpistemicClass, Modality, SourceClass};

/// Facts supplied by the semantic owner after its permission/lifecycle checks.
/// Missing facts cannot satisfy an explicitly requested constraint.
pub struct QueryFacts<'a> {
    pub authority: AuthorityClass,
    pub entities: &'a [EntityRef],
    pub source_classes: &'a [SourceClass],
    pub modality: Modality,
    pub cognitive_role: Option<&'a str>,
    pub formation_mode: Option<&'a str>,
    pub epistemic_class: Option<EpistemicClass>,
}

impl QueryConstraints {
    pub fn matches_common(&self, facts: QueryFacts<'_>) -> bool {
        self.authority
            .is_none_or(|authority| authority == facts.authority)
            && self
                .entity_requirements
                .iter()
                .all(|entity| facts.entities.contains(entity))
            && (self.source_classes_include.is_empty()
                || self
                    .source_classes_include
                    .iter()
                    .any(|source| facts.source_classes.contains(source)))
            && !self
                .source_classes_exclude
                .iter()
                .any(|source| facts.source_classes.contains(source))
            && (self.modalities.is_empty() || self.modalities.contains(&facts.modality))
            && matches_label(&self.cognitive_roles_include, facts.cognitive_role)
            && matches_label(&self.formation_modes_include, facts.formation_mode)
            && matches_label(
                &self.evidence_classes,
                facts.epistemic_class.map(EpistemicClass::as_str),
            )
    }
}

fn matches_label(requested: &[String], actual: Option<&str>) -> bool {
    requested.is_empty()
        || actual.is_some_and(|actual| requested.iter().any(|value| value == actual))
}
