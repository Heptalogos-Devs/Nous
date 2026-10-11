//! Immutable on-disk serving generations shared by runtime and cognitive contributors.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod policy;
pub use policy::epa::{EPA_POLICY, EpaPolicy};
pub use policy::wave::*;
mod mechanisms;
pub use mechanisms::*;
mod assets;
mod concept;
pub use assets::reclamation::ReclamationReport;
pub use concept::generation::{ConceptGeneration, ConceptRecord};
mod embedding;
mod material;
pub use embedding::{EmbeddingCursor, EmbeddingNeed, EmbeddingPage};
mod projection;
pub use projection::{CognitiveProjectionInput, OwnedTopologyProjection};
mod observation;
mod provider;
pub mod reference;
pub use observation::{QueryObservation, QueryTemporalContext};
mod query;
pub use query::PreparedQuerySignals;
mod activated_routes;
mod topology_lane;
mod vcp;
pub use material::*;
pub use policy::vcp::*;
pub use vcp::adapter::*;
pub use vcp::generation::*;
pub use vcp::graph::*;
pub use vcp::index::*;
pub use vcp::material::*;
pub use vcp::observation::*;
pub use vcp::readout::*;

use nous_core::*;
use nous_object_store::ObjectStore;
use nous_persistence::{AuthorityStore, ServingRecord};
pub use provider::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

const EPISODE_SYNOPSIS: nous_configuration::ConfigKey<nous_persistence::EpisodeTextBudget> =
    nous_configuration::ConfigKey::new("episode.synopsis");

pub const LEXICAL_ENABLED_KEY: nous_configuration::ConfigKey<bool> =
    nous_configuration::ConfigKey::new("serving.lexical.enabled");
pub const DENSE_ENABLED_KEY: nous_configuration::ConfigKey<bool> =
    nous_configuration::ConfigKey::new("serving.dense.enabled");
pub const TOPOLOGY_ENABLED_KEY: nous_configuration::ConfigKey<bool> =
    nous_configuration::ConfigKey::new("serving.topology.enabled");

pub const RETIRED_GRACE_SECONDS: nous_configuration::ConfigKey<u64> =
    nous_configuration::ConfigKey::new("serving.retired_grace_seconds");

pub const LEXICAL_WRITER_BYTES: nous_configuration::ConfigKey<usize> =
    nous_configuration::ConfigKey::new("serving.lexical.writer_memory_bytes");
pub const MINIMUM_LEXICAL_WRITER_BYTES: usize = 15_000_000;
pub const ABSOLUTE_LEXICAL_WRITER_BYTES: usize = 512 * 1024 * 1024;

pub fn register_configuration(
    registry: &mut nous_configuration::ConfigRegistryBuilder,
) -> Result<()> {
    policy::epa::register_configuration(registry)?;
    policy::vcp::register_configuration(registry)?;
    for (key, description, default) in [
        (
            LEXICAL_ENABLED_KEY,
            "Whether lexical Serving is enabled.",
            true,
        ),
        (DENSE_ENABLED_KEY, "Whether dense Serving is enabled.", true),
        (
            TOPOLOGY_ENABLED_KEY,
            "Whether experimental topology Serving is enabled.",
            false,
        ),
    ] {
        registry.register(
            key,
            "serving",
            description,
            default,
            nous_configuration::ConfigExposure::Developer,
            nous_configuration::ConfigScopePolicy::SystemOnly,
            nous_configuration::ConfigApplyMode::RestartProcess,
            nous_configuration::ConfigSemanticEffect::Operational,
            |_: &bool| Ok(()),
        )?;
    }
    use nous_configuration::*;
    registry.register(
        RETIRED_GRACE_SECONDS,
        "serving",
        "Grace period before reclaiming unpinned retired artifacts.",
        300,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::Operational,
        |value| {
            if *value <= 604800 {
                Ok(())
            } else {
                Err(Error::Invalid("retired grace exceeds seven days".into()))
            }
        },
    )?;
    registry.bounds(RETIRED_GRACE_SECONDS, 0, 604800, Some("seconds"))?;
    registry.register(
        LEXICAL_WRITER_BYTES,
        "serving",
        "Lexical build writer memory budget.",
        MINIMUM_LEXICAL_WRITER_BYTES,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::Operational,
        |value| {
            if (MINIMUM_LEXICAL_WRITER_BYTES..=ABSOLUTE_LEXICAL_WRITER_BYTES).contains(value) {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "lexical writer memory exceeds supported bounds".into(),
                ))
            }
        },
    )?;
    registry.bounds(
        LEXICAL_WRITER_BYTES,
        MINIMUM_LEXICAL_WRITER_BYTES,
        ABSOLUTE_LEXICAL_WRITER_BYTES,
        Some("bytes"),
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServingOptions {
    pub root: PathBuf,
    pub lexical: bool,
    pub dense: bool,
    pub topology: bool,
    pub memory_enabled: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectionCapabilities {
    pub memory: bool,
}

#[derive(Clone)]
pub struct ServingService {
    pub store: AuthorityStore,
    pub objects: ObjectStore,
    pub material: nous_material::MaterialService,
    pub memory: Option<nous_memory::MemoryService>,
    pub configuration: nous_configuration::ConfigurationService,
    pub publisher: ServingPublisher,
    pub options: ServingOptions,
    read_gate: Arc<tokio::sync::RwLock<()>>,
    query_readers: Arc<std::sync::Mutex<Vec<std::sync::Weak<query::ServingQuery>>>>,
    embedding: Arc<std::sync::OnceLock<Arc<dyn TextEmbeddingProvider>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectionStatus {
    pub generations: BTreeMap<String, ServingGenerationId>,
    pub rebuilt: Vec<String>,
    pub reopened: Vec<String>,
    pub degradation: Vec<Degradation>,
}

impl ServingService {
    pub fn embedding(&self) -> Option<&Arc<dyn TextEmbeddingProvider>> {
        self.embedding.get()
    }

    /// Host material binding is established once during process startup.
    pub fn initialize_embedding(&self, config: StoredEmbeddingConfig) -> Result<()> {
        let provider = Arc::new(StoredEmbeddingProvider::new(self.store.clone(), config)?);
        self.embedding
            .set(provider)
            .map_err(|_| Error::Conflict("host embedding already initialized".into()))
    }

    pub fn new(
        material: nous_material::MaterialService,
        memory: Option<nous_memory::MemoryService>,
        options: ServingOptions,
        embedding: Option<Arc<dyn TextEmbeddingProvider>>,
        configuration: nous_configuration::ConfigurationService,
    ) -> Result<Self> {
        std::fs::create_dir_all(&options.root).map_err(assets::files::io)?;
        Ok(Self {
            store: material.store.clone(),
            objects: material.objects.clone(),
            material,
            memory,
            configuration,
            options,
            embedding: {
                let slot = std::sync::OnceLock::new();
                if let Some(provider) = embedding {
                    let _ = slot.set(provider);
                }
                Arc::new(slot)
            },
            publisher: ServingPublisher::default(),
            read_gate: Arc::new(tokio::sync::RwLock::new(())),
            query_readers: Default::default(),
        })
    }
}
