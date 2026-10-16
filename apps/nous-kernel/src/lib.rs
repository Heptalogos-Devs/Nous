//! Static composition of Subject Core, Cognitive Runtime and optional Memory.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod context;
mod longitudinal;
mod source_read;
pub use longitudinal::ExperienceSegmentationResult;
pub mod transport;

use nous_configuration::{
    ConfigRegistryBuilder, ConfigurationService, ProcessCapabilities, process_capabilities,
};
use nous_core::*;
use nous_material::MaterialService;
use nous_memory::MemoryService;
use nous_object_store::ObjectStore;
use nous_persistence::AuthorityStore;
use nous_retrieval::{ServingOptions, ServingService, TextEmbeddingProvider};
use nous_runtime::CognitiveRuntimeService;
use nous_subject::SubjectCoreService;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct NousRuntime {
    pub clocks: nous_runtime::SubjectClockService,
    pub configuration: ConfigurationService,
    pub store: AuthorityStore,
    pub subjects: SubjectCoreService,
    pub self_cognition: Option<nous_self::SelfService>,
    pub motivation: Option<nous_motivation::MotivationService>,
    pub social_cognition: Option<nous_social::SocialService>,
    pub cognition: CognitiveRuntimeService,
    pub memory: Option<MemoryService>,
    pub process_capabilities: ProcessCapabilities,
    pub material: MaterialService,
    pub serving: ServingService,
}

pub struct RuntimeOptions {
    pub postgres_url: String,
    pub max_connections: u32,
    pub acquire_timeout_ms: u64,
    pub object_root: String,
    pub serving_options: ServingOptions,
    pub embedding: Option<Arc<dyn TextEmbeddingProvider>>,
    pub stored_embedding: Option<nous_retrieval::StoredEmbeddingConfig>,
    pub configuration: nous_configuration::ConfigDeployment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub ready: bool,
    pub capabilities: Vec<CapabilityStatus>,
}

impl NousRuntime {
    pub async fn open(options: RuntimeOptions) -> Result<Self> {
        Self::open_with_clock(options, Arc::new(nous_runtime::SystemCognitiveClock)).await
    }

    pub async fn open_with_clock(
        options: RuntimeOptions,
        clock: Arc<dyn nous_runtime::CognitiveClock>,
    ) -> Result<Self> {
        let store = AuthorityStore::connect(
            &options.postgres_url,
            options.max_connections,
            std::time::Duration::from_millis(options.acquire_timeout_ms),
        )
        .await?;
        store.migrate().await?;
        let configuration =
            ConfigurationService::open(store.clone(), options.configuration).await?;
        let system_snapshot = configuration.active_system_snapshot()?;
        let process_capabilities = process_capabilities(&system_snapshot)?;
        let resident_limit = system_snapshot.get(nous_runtime::RESIDENT_LIMIT_KEY)?;
        let clocks = nous_runtime::SubjectClockService::open(store.clone(), clock.clone()).await?;
        let objects = ObjectStore::open(&options.object_root).await?;
        let subjects = SubjectCoreService::with_clock(
            store.clone(),
            objects.clone(),
            configuration.clone(),
            clock.clone(),
        );
        let cognition = CognitiveRuntimeService::with_clock(
            store.clone(),
            resident_limit,
            configuration.clone(),
            clock,
        )?
        .with_semantic_clock(Arc::new(clocks.clone()));
        let material = MaterialService::new(
            store.clone(),
            objects.clone(),
            cognition.clone(),
            system_snapshot.get(nous_material::MAX_UPLOAD_BYTES)?,
        )?;
        let self_cognition = process_capabilities.self_cognition.then(|| {
            nous_self::SelfService::new(
                store.clone(),
                material.clone(),
                objects.clone(),
                cognition.clone(),
            )
        });
        let mut serving_options = options.serving_options;
        serving_options.memory_enabled = process_capabilities.memory;
        serving_options.lexical = system_snapshot.get(nous_retrieval::LEXICAL_ENABLED_KEY)?;
        serving_options.dense = system_snapshot.get(nous_retrieval::DENSE_ENABLED_KEY)?;
        serving_options.topology = system_snapshot.get(nous_retrieval::TOPOLOGY_ENABLED_KEY)?;
        let memory = process_capabilities.memory.then(|| {
            MemoryService::new(
                store.clone(),
                cognition.clone(),
                material.clone(),
                configuration.clone(),
            )
        });
        let social_cognition = process_capabilities.social_cognition.then(|| {
            nous_social::SocialService::new(
                store.clone(),
                material.clone(),
                objects.clone(),
                cognition.clone(),
                Arc::new(source_read::SourceRoots {
                    store: store.clone(),
                    material: material.clone(),
                    memory: memory.clone(),
                    self_cognition: self_cognition.clone(),
                }),
            )
        });
        let motivation = process_capabilities.motivation.then(|| {
            nous_motivation::MotivationService::new(
                store.clone(),
                material.clone(),
                objects.clone(),
                cognition.clone(),
                clocks.clone(),
                Arc::new(source_read::SourceRoots {
                    store: store.clone(),
                    material: material.clone(),
                    memory: memory.clone(),
                    self_cognition: self_cognition.clone(),
                }),
            )
        });
        let serving = ServingService::new(
            store.clone(),
            material.clone(),
            memory.clone(),
            serving_options,
            match options.stored_embedding {
                Some(config) => Some(Arc::new(nous_retrieval::StoredEmbeddingProvider::new(
                    store.clone(),
                    config,
                )?)),
                None => options.embedding,
            },
            configuration.clone(),
        )?;
        Ok(Self {
            clocks,
            configuration,
            store,
            subjects,
            self_cognition,
            social_cognition,
            motivation,
            cognition,
            memory,
            process_capabilities,
            material,
            serving,
        })
    }

    pub fn require_self(&self) -> Result<&nous_self::SelfService> {
        self.self_cognition.as_ref().ok_or_else(|| {
            DomainError::new(
                DomainErrorCode::CapabilityUnavailable,
                "Self MicroSystem is disabled",
            )
            .with_context("capability", "self_cognition")
            .into()
        })
    }
    pub fn require_social(&self) -> Result<&nous_social::SocialService> {
        self.social_cognition.as_ref().ok_or_else(|| {
            DomainError::new(
                DomainErrorCode::CapabilityUnavailable,
                "Social MicroSystem is disabled",
            )
            .with_context("capability", "social_cognition")
            .into()
        })
    }
    pub fn require_motivation(&self) -> Result<&nous_motivation::MotivationService> {
        self.motivation.as_ref().ok_or_else(|| {
            DomainError::new(
                DomainErrorCode::CapabilityUnavailable,
                "Motivation MicroSystem is disabled",
            )
            .with_context("capability", "motivation")
            .into()
        })
    }
    pub fn require_memory(&self) -> Result<&MemoryService> {
        self.memory.as_ref().ok_or_else(|| {
            DomainError::new(
                DomainErrorCode::CapabilityUnavailable,
                "Memory MicroSystem is disabled",
            )
            .with_context("capability", "memory")
            .into()
        })
    }

    pub(crate) fn query_owners(
        &self,
        capabilities: nous_configuration::SubjectCapabilities,
    ) -> Vec<&dyn nous_runtime::CognitiveContributor> {
        let mut owners: Vec<&dyn nous_runtime::CognitiveContributor> = vec![&self.material];
        if capabilities.memory
            && let Some(owner) = &self.memory
        {
            owners.push(owner);
        }
        if capabilities.self_cognition
            && let Some(owner) = &self.self_cognition
        {
            owners.push(owner);
        }
        if capabilities.social_cognition
            && let Some(owner) = &self.social_cognition
        {
            owners.push(owner);
        }
        if capabilities.motivation
            && let Some(owner) = &self.motivation
        {
            owners.push(owner);
        }
        owners
    }
    pub async fn query(&self, query: CognitiveQuery) -> Result<CognitiveQueryResult> {
        Ok(self.execute_query(query, None).await?.result)
    }
    pub async fn bind_query_with_snapshot(
        &self,
        query: CognitiveQuery,
        config: nous_configuration::ConfigSnapshot,
    ) -> Result<nous_runtime::BoundQuery> {
        self.bind_query_with_snapshot_authorized(query, config, &[])
            .await
    }
    pub async fn bind_query_with_snapshot_authorized(
        &self,
        mut query: CognitiveQuery,
        config: nous_configuration::ConfigSnapshot,
        social_authorized_refs: &[CognitiveRef],
    ) -> Result<nous_runtime::BoundQuery> {
        let caps = self.subjects.subject(query.subject).await?.capabilities;
        query.projection.domains.retain(|d| match d.owner() {
            CognitiveDomain::Memory => caps.memory,
            CognitiveDomain::SelfCognition => caps.self_cognition,
            CognitiveDomain::Motivation => caps.motivation && !social_authorized_refs.is_empty(),
            CognitiveDomain::SocialCognition => {
                caps.social_cognition && !social_authorized_refs.is_empty()
            }
            _ => true,
        });
        if query
            .projection
            .domains
            .iter()
            .any(|d| d.owner() == CognitiveDomain::Memory)
        {
            self.require_memory()?;
        }
        if query
            .projection
            .domains
            .iter()
            .any(|d| d.owner() == CognitiveDomain::SelfCognition)
        {
            self.require_self()?;
        }
        if query
            .projection
            .domains
            .iter()
            .any(|d| d.owner() == CognitiveDomain::SocialCognition)
        {
            self.require_social()?;
        }
        if query.projection.domains.is_empty() {
            return Err(DomainError::new(
                DomainErrorCode::CapabilityUnavailable,
                "Requested cognition domains are not provisioned",
            )
            .into());
        }
        if query.temporal_frame.clock_now == chrono::DateTime::<chrono::Utc>::UNIX_EPOCH {
            query.temporal_frame.clock_now = self.cognition.now(query.subject);
        }
        let historical = query.temporal_frame.authority_view != AuthorityView::Current
            || query.temporal_frame.revision_view != RevisionView::Current;
        let view = if historical {
            let at = match query.temporal_frame.authority_view {
                AuthorityView::AsOf(at) => at,
                AuthorityView::Current => query.temporal_frame.clock_now,
            };
            let view = self
                .historical_authority_view(query.subject, at, query.temporal_frame.revision_view)
                .await?;
            Some(Arc::new(view))
        } else {
            None
        };
        let mut bound = self
            .cognition
            .bind_query_with_authority_view(query, config, view, self)
            .await?;
        bound.social_authorized_refs = social_authorized_refs.to_vec();
        Ok(bound)
    }
    pub async fn historical_authority_view(
        &self,
        subject: SubjectId,
        at: chrono::DateTime<chrono::Utc>,
        revision_view: RevisionView,
    ) -> Result<HistoricalAuthoritySnapshot> {
        let capabilities = self.subjects.subject(subject).await?.capabilities;
        let mut view = if let Some(memory) = self.memory.as_ref().filter(|_| capabilities.memory) {
            memory.project_as_of(subject, at, revision_view).await?
        } else {
            HistoricalAuthoritySnapshot::empty(subject, at, revision_view)
        };
        if capabilities.social_cognition {
            view.cognition.extend(
                self.require_social()?
                    .project_as_of(subject, at, revision_view)
                    .await?,
            );
        }
        if capabilities.motivation {
            view.cognition.extend(
                self.require_motivation()?
                    .project_as_of(subject, at, revision_view)
                    .await?,
            );
        }
        if capabilities.self_cognition {
            view.cognition.extend(
                self.require_self()?
                    .project_as_of(subject, at, revision_view)
                    .await?,
            );
        }
        let material = self.material.project_as_of(subject, at).await?;
        view.material_visibility = material.known_references;
        view.material_documents = material.document_references;
        view.entity_bindings = material.entity_bindings;
        view.lexical_visibility.extend(material.lexical_visibility);
        view.lexical_visibility
            .sort_by(|a, b| a.lexical_ref.cmp(&b.lexical_ref));
        view.lexical_visibility
            .dedup_by(|a, b| a.lexical_ref == b.lexical_ref);
        view.refresh_digest()?;
        Ok(view)
    }
    pub async fn execute_query(
        &self,
        query: CognitiveQuery,
        pool_limit: Option<usize>,
    ) -> Result<nous_runtime::QueryExecution> {
        let snapshot = self.configuration.snapshot_for_subject(query.subject)?;
        let bound = Box::pin(self.bind_query_with_snapshot(query, snapshot)).await?;
        Box::pin(self.execute_bound_query(bound, pool_limit)).await
    }
    pub async fn execute_bound_query(
        &self,
        mut bound: nous_runtime::BoundQuery,
        pool_limit: Option<usize>,
    ) -> Result<nous_runtime::QueryExecution> {
        if bound.source_query.is_exact_read() {
            self.cognition.expire_query_leases()?;
            let plan = nous_runtime::QueryPlan::for_bound_query(&bound);
            let capabilities = self
                .subjects
                .subject(bound.source_query.subject)
                .await?
                .capabilities;
            return self
                .cognition
                .execute_query_with_plan(
                    bound,
                    nous_runtime::CognitiveContributors {
                        shared: None,
                        owners: self.query_owners(capabilities),
                    },
                    plan,
                    pool_limit,
                )
                .await;
        }
        if bound.selected_embedding_space.is_none() {
            bound.selected_embedding_space =
                self.serving.embedding().map(|provider| provider.space());
        }
        self.cognition.expire_query_leases()?;
        let plan = nous_runtime::QueryPlan::for_bound_query(&bound);
        let subject_capabilities = self
            .subjects
            .subject(bound.source_query.subject)
            .await?
            .capabilities;
        let (projection, serving_query): (_, Arc<dyn nous_runtime::QueryActivationView>) =
            if let Some(view) = &bound.activation_view {
                (nous_retrieval::ProjectionStatus::default(), view.clone())
            } else {
                let (projection, view) = self.serving.prepare_query(&bound, &plan).await?;
                (projection, view)
            };
        let mut execution = self
            .cognition
            .execute_query_with_plan(
                bound,
                nous_runtime::CognitiveContributors {
                    shared: Some(serving_query.provider()),
                    owners: self.query_owners(subject_capabilities),
                },
                plan,
                pool_limit,
            )
            .await?;
        execution.result.generation = serving_query.generation_trace();
        execution.bound.activation_view = Some(serving_query);
        let result = &mut execution.result;
        result.degradation.extend(projection.degradation);
        if !result.degradation.is_empty() && result.status != QueryStatus::Partial {
            result.status = if result
                .degradation
                .iter()
                .any(|d| d.code == "required_external_authority_unresolved")
            {
                QueryStatus::Partial
            } else {
                QueryStatus::Degraded
            };
        }
        Ok(execution)
    }

    pub async fn status(&self) -> RuntimeStatus {
        let capabilities = vec![
            CapabilityStatus {
                capability_id: "subject_core".into(),
                status: Readiness::Ready,
                reason: None,
            },
            CapabilityStatus {
                capability_id: "cognitive_runtime".into(),
                status: Readiness::Ready,
                reason: None,
            },
            CapabilityStatus {
                capability_id: "memory".into(),
                status: if self.memory.is_some() {
                    Readiness::Ready
                } else {
                    Readiness::Unavailable
                },
                reason: self
                    .memory
                    .is_none()
                    .then(|| "disabled in composition".into()),
            },
            CapabilityStatus {
                capability_id: "motivation".into(),
                status: if self.motivation.is_some() {
                    Readiness::Ready
                } else {
                    Readiness::Unavailable
                },
                reason: self
                    .motivation
                    .is_none()
                    .then(|| "disabled in composition".into()),
            },
        ];
        RuntimeStatus {
            ready: self.store.check().await.is_ok(),
            capabilities,
        }
    }
}

/// Finalize the catalog through the same path for startup and offline validation.
pub fn configuration_catalog(
    core_descriptors: Vec<nous_configuration::ConfigDescriptor>,
) -> Result<nous_configuration::ConfigRegistry> {
    let mut registry = ConfigRegistryBuilder::new();
    nous_configuration::register_configuration(&mut registry)?;
    nous_runtime::register_configuration(&mut registry)?;
    nous_memory::register_configuration(&mut registry)?;
    nous_motivation::register_configuration(&mut registry)?;
    nous_material::register_configuration(&mut registry)?;
    nous_retrieval::register_configuration(&mut registry)?;
    nous_retrieval::register_wave_configuration(&mut registry)?;
    for descriptor in core_descriptors {
        registry.import(descriptor)?;
    }
    registry.finish()
}
