// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
pub(super) fn revision_header(value: &nous_memory::JournalView) -> p::CognitionRevisionHeader {
    super::cognition::cognition_revision_header_proto(&value.revision.header)
}

pub(super) fn revision_proto(value: &nous_memory::JournalView) -> p::JournalRevision {
    let revision = &value.revision;
    p::JournalRevision {
        header: Some(revision_header(value)),
        revision_intent: revision.revision_intent.clone(),
        title: revision.title.clone(),
        temporal_scope: Some(temporal_proto(&revision.temporal_scope)),
        narrative: revision.narrative.clone(),
        points: value
            .points
            .iter()
            .cloned()
            .enumerate()
            .map(|(ordinal, point)| p::JournalPoint {
                ordinal: ordinal as i32,
                role: point.role.as_str().into(),
                text: point.text,
                basis: point.basis.into_iter().map(basis_proto).collect(),
            })
            .collect(),
        sources: value.sources.iter().cloned().map(to_ref).collect(),
    }
}

pub(super) fn journal_revision_proto(value: &nous_memory::JournalView) -> p::JournalRevision {
    revision_proto(value)
}

pub(super) fn journal_response(value: nous_memory::JournalView) -> p::JournalResponse {
    p::JournalResponse {
        journal: Some(view(value)),
        degradation: Vec::new(),
    }
}

pub(super) fn view(value: nous_memory::JournalView) -> p::Journal {
    p::Journal {
        header: Some(super::cognition::cognition_header_proto(&value.header)),
        revision: Some(revision_proto(&value)),
    }
}
