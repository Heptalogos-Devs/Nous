// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{Error, Result};

/// Physical assets, independent of semantic query EvidenceFamily lanes.
#[derive(Clone, Copy)]
pub(super) enum AssetFamily {
    Lexical,
    Dense,
    Topology,
    Concept,
}

impl AssetFamily {
    pub const ALL: [Self; 4] = [Self::Lexical, Self::Dense, Self::Topology, Self::Concept];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Dense => "dense",
            Self::Topology => "topology",
            Self::Concept => "concept",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|family| family.as_str() == value)
            .ok_or_else(|| Error::Invalid("unknown physical Serving family".into()))
    }
    pub fn implementation(self) -> &'static str {
        match self {
            Self::Lexical => "tantivy",
            Self::Dense => "usearch",
            Self::Topology => "cognitive-profile-assets",
            Self::Concept => "shared-semantic-concepts",
        }
    }
}
