//! Static composition of Subject Core, Cognitive Runtime and optional Memory.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod context;
mod longitudinal;
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
    pub configuration: ConfigurationService,
    pub store: AuthorityStore,
    pub subjects: SubjectCoreService,
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
    pub deployment_document: serde_json::Value,
    pub core_descriptors: Vec<nous_configuration::ConfigDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub ready: bool,
    pub capabilities: Vec<CapabilityStatus>,
}

impl NousRuntime {
    pub async fn materialize(
        &self,
        subject: SubjectId,
        request: nous_material::MaterializeRequest,
    ) -> Result<nous_material::MaterializedEvidence> {
        self.material.materialize(subject, request).await
    }
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
        let registry = configuration_catalog(options.core_descriptors)?;
        let configuration =
            ConfigurationService::open(store.clone(), registry, options.deployment_document)
                .await?;
        let system_snapshot = configuration.active_system_snapshot()?;
        let process_capabilities = process_capabilities(&system_snapshot)?;
        let resident_limit = system_snapshot.get(nous_runtime::RESIDENT_LIMIT_KEY)?;
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
        )?;
        let material = MaterialService::new(
            store.clone(),
            objects.clone(),
            cognition.clone(),
            system_snapshot.get(nous_material::MAX_UPLOAD_BYTES)?,
        )?;
        let mut serving_options = options.serving_options;
        serving_options.memory_enabled = process_capabilities.memory;
        serving_options.lexical = system_snapshot.get(nous_retrieval::LEXICAL_ENABLED_KEY)?;
        serving_options.dense = system_snapshot.get(nous_retrieval::DENSE_ENABLED_KEY)?;
        serving_options.topology = system_snapshot.get(nous_retrieval::TOPOLOGY_ENABLED_KEY)?;
        let memory = process_capabilities
            .memory
            .then(|| MemoryService::new(material.clone(), configuration.clone()));
        let serving = ServingService::new(
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
            configuration,
            store,
            subjects,
            cognition,
            memory,
            process_capabilities,
            material,
            serving,
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

    pub async fn query(&self, query: CognitiveQuery) -> Result<CognitiveQueryResult> {
        Ok(self.execute_query(query, None).await?.result)
    }
    pub async fn bind_query_with_snapshot(
        &self,
        mut query: CognitiveQuery,
        config: nous_configuration::ConfigSnapshot,
    ) -> Result<nous_runtime::BoundQuery> {
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
        self.cognition
            .bind_query_with_authority_view(query, config, view)
            .await
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
                        material: Some(&self.material),
                        shared: None,
                        memory: self
                            .memory
                            .as_ref()
                            .filter(|_| capabilities.memory)
                            .map(|memory| memory as &dyn nous_runtime::CognitiveContributor),
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
        self.serving
            .reclaim_retired(
                bound.source_query.subject,
                std::time::Duration::from_secs(
                    bound
                        .config_snapshot
                        .get(nous_retrieval::RETIRED_GRACE_SECONDS)?,
                ),
            )
            .await?;
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
                    material: Some(&self.material),
                    shared: Some(serving_query.provider()),
                    memory: subject_capabilities
                        .memory
                        .then(|| {
                            self.memory
                                .as_ref()
                                .map(|memory| memory as &dyn nous_runtime::CognitiveContributor)
                        })
                        .flatten(),
                },
                plan,
                pool_limit,
            )
            .await?;
        execution.result.generation = serving_query.generation_trace();
        execution.read_lease = Some(serving_query);
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
        let mut capabilities = vec![
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
        ];
        if let Some(memory) = &self.memory {
            capabilities.extend(memory.status().await.capabilities);
        }
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
    nous_material::register_configuration(&mut registry)?;
    nous_retrieval::register_configuration(&mut registry)?;
    nous_retrieval::register_wave_configuration(&mut registry)?;
    for descriptor in core_descriptors {
        registry.import(descriptor)?;
    }
    registry.finish()
}
