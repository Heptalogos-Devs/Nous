//! Shared Cognition read and lifecycle state for Memory Authority.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::service::schema::SchemaView;

mod lifecycle;
mod read;
pub(in crate::service) use read::CognitionSelection;

#[derive(Debug, Clone)]
pub enum CognitionView {
    Memory(MemoryView),
    CognitiveSchema(SchemaView),
    Episode(EpisodeView),
    Journal(JournalView),
}

impl CognitionView {
    pub fn header(&self) -> &CognitionHeader {
        match self {
            Self::Memory(view) => &view.header,
            Self::CognitiveSchema(view) => &view.header,
            Self::Episode(view) => &view.header,
            Self::Journal(view) => &view.header,
        }
    }

    pub fn revision_header(&self) -> &CognitionRevisionHeader {
        match self {
            Self::Memory(view) => &view.revision.header,
            Self::CognitiveSchema(view) => &view.revision.header,
            Self::Episode(view) => &view.revision.header,
            Self::Journal(view) => &view.revision.header,
        }
    }
}
