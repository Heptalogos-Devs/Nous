// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{Error, Result};

/// Configuration paths that can change a family's built bytes.
/// Query/readout policies and serving request switches are deliberately absent.
pub fn family_config_keys(family: &str) -> Option<Vec<&'static str>> {
    let synopsis = crate::EPISODE_SYNOPSIS.path();
    Some(match family {
        "lexical" => vec![synopsis],
        "dense" => vec![crate::EPA_POLICY.path(), synopsis],
        "concept" => Vec::new(),
        "topology" => {
            let mut keys = vec![synopsis, crate::VCP_ASSETS.path()];
            keys.extend(crate::WAVE_BUILD_CONFIG_KEYS.iter().copied());
            keys
        }
        _ => return None,
    })
}

/// Physical families potentially affected by a set of changed config paths.
/// Exact path membership avoids guessing from key prefixes.
pub fn affected_families<K: AsRef<str>>(keys: &[K]) -> Vec<&'static str> {
    ["lexical", "dense", "concept", "topology"]
        .into_iter()
        .filter(|family| {
            family_config_keys(family).is_some_and(|family_keys| {
                keys.iter().any(|key| family_keys.contains(&key.as_ref()))
            })
        })
        .collect()
}

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
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "unknown physical Serving family",
                )
            })
    }
    pub fn implementation(self) -> &'static str {
        match self {
            Self::Lexical => "tantivy",
            Self::Dense => "usearch",
            Self::Topology => "cognitive-profile-assets",
            Self::Concept => "shared-semantic-concepts",
        }
    }

    /// Deliberate per-family physical format/build-semantic identity.
    /// Query readout and repository source changes do not alter these values.
    pub fn implementation_revision(
        self,
        profile: Option<nous_runtime::CognitiveProfile>,
    ) -> &'static str {
        match self {
            Self::Lexical => "lexical-v1",
            Self::Dense => "dense-v1",
            Self::Concept => "concept-v1",
            Self::Topology => match profile {
                Some(nous_runtime::CognitiveProfile::VcpDtsc)
                | Some(nous_runtime::CognitiveProfile::VcpRiverMemo) => "vcp-topology-v1",
                _ => "native-topology-v2",
            },
        }
    }
}
