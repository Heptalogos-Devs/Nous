pub mod administration;
pub mod backup;
pub mod bundle_format;
pub mod cognition;
mod composition;
pub mod import_format;
mod lifecycle;
pub mod retrieval;
pub mod materials;

use nous_domain::*;
use nous_foundation::clock::Clock;
use nous_foundation::configuration::Configuration;
use nous_foundation::content::ObjectStore;
use nous_foundation::instance;
use nous_foundation::instance::InstancePaths;
use nous_foundation::model_calls::ModelCalls;
use nous_foundation::models::ModelRuntime;
use nous_foundation::network;
use nous_foundation::tasks::Tasks;
use nous_foundation::{identity::Identity, secret::Secrets};
use nous_retrieval::RetrievalIndex;
pub use nous_storage::OperationReceipt;
use nous_storage::{BeginOperation, Database, operation_key};
use serde_json::{Value, json};
use std::sync::Arc;

/// The shared entrypoint used by every transport. Domain rules live here;
/// durable commit and queue mechanics live in nous-storage.
#[derive(Clone)]
pub struct App {
    database: Database,
    objects: ObjectStore,
    sources:materials::sources::SourceReader,
    paths: Arc<InstancePaths>,
    config: Arc<instance::InstanceConfig>,
    model: Arc<ModelRuntime>,
    pub(crate) configuration: Configuration,
    tasks: Tasks,
    calls: ModelCalls,
    identity: Identity,
    pub(crate) platform: nous_foundation::platform::Platform,
    pub(crate) media: nous_foundation::media::Media,
    retrieval: RetrievalIndex,
    index_gate: Arc<tokio::sync::Mutex<()>>,
    semantic_cache: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<String, Arc<nous_retrieval::serving::ServingGeneration>>,
        >,
    >,
}

impl App {
    pub async fn open(paths: InstancePaths) -> Result<Self> {
        let paths = Arc::new(paths);
        let database = Database::open(&paths.database, Clock::operation_snapshot).await?;
        let objects = ObjectStore::new(paths.clone());
        let mut config = if paths.config.exists() {
            instance::InstanceConfig::load(&paths)
                .map_err(|error| Error::invalid(error.to_string()))?
        } else {
            instance::InstanceConfig::default()
        };
        let clock = Clock;
        let roles = composition::model_roles()?;
        let registry = composition::configuration_registry(&roles)?;
        let configuration =
            Configuration::new(database.clone(), registry, &serde_json::to_value(&config)?).await?;
        let desired = configuration.snapshot(&ConfigScope::Instance, true).await?;
        config.server = desired.get(nous_foundation::configuration::SERVER)?;
        config.auth = desired.get(nous_foundation::configuration::AUTH)?;
        config.network = desired.get(nous_foundation::configuration::NETWORK)?;
        config.observability = desired.get(nous_foundation::observability::OBSERVABILITY)?;
        config.tasks = desired.get(nous_foundation::tasks::TASK_RUNTIME)?;
        config.platform = desired.get(nous_foundation::platform::PLATFORM)?;
        let platform = nous_foundation::platform::Platform::new(
            paths.as_ref().clone(),
            config.platform.clone(),
        )?;
        let media = nous_foundation::media::Media::new(
            objects.clone(),
            configuration.clone(),
            platform.clone(),
        );
        let secrets = Secrets::new(paths.clone());
        secrets.reconcile(&database).await?;
        let identity = Identity::new(database.clone(), secrets.clone(), clock);
        identity
            .initialize_bootstrap(&config.auth.admin_token)
            .await?;
        let network = network::NetworkClients::new(&config.network)
            .map_err(|error| Error::new(ErrorCode::DependencyUnavailable, error.to_string()))?;
        let budgets =
            nous_foundation::models::ModelBudgets::new(configuration.clone(), roles.clone());
        let prompts = nous_foundation::prompts::Prompts::new(
            paths.as_ref().clone(),
            database.clone(),
            roles.definitions().filter_map(|role| role.prompt.clone()),
        );
        let sources=materials::sources::SourceReader::new(database.clone(),objects.clone(),media.clone());
        let model = Arc::new(
            ModelRuntime::new(
                secrets,
                configuration.clone(),
                roles,
                prompts,
                network.clone(),
                Arc::new(sources.clone()),
            )
            .map_err(|error| Error::invalid(error.to_string()))?,
        );
        let index_path = paths
            .resolve("indexes/lexical")
            .map_err(|error| Error::invalid(error.to_string()))?;
        let retrieval = nous_foundation::execution::blocking(
            &tokio_util::sync::CancellationToken::new(),
            move |_| {
                RetrievalIndex::open(index_path).map_err(|error| {
                    Error::new(ErrorCode::DependencyUnavailable, error.to_string())
                        .with_cause(error.as_ref())
                })
            },
        )
        .await?;
        let tasks = Tasks::new(database.clone(), clock, config.tasks.clone())?;
        let calls = ModelCalls::new(database.clone(), tasks.clone(), clock, budgets);
        Ok(Self {
            database,
            objects,
            sources,
            paths,
            config: Arc::new(config),
            model,
            configuration,
            tasks,
            calls,
            identity,
            platform,
            media,
            retrieval,
            index_gate: Arc::new(tokio::sync::Mutex::new(())),
            semantic_cache: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        })
    }

    pub fn bootstrap_config(&self) -> &instance::InstanceConfig {
        &self.config
    }
    pub fn paths(&self) -> &InstancePaths {
        &self.paths
    }
    pub async fn close(&self) {
        self.platform.shutdown().await;
        let index = self.retrieval.clone();
        if let Err(error) = nous_foundation::execution::blocking(
            &tokio_util::sync::CancellationToken::new(),
            move |_| {
                index.close().map_err(|error| {
                    Error::new(ErrorCode::DependencyUnavailable, error.to_string())
                        .with_cause(error.as_ref())
                })
            },
        )
        .await
        {
            tracing::warn!(error=?error, "index close failed");
        }
        self.database.close().await;
    }

    pub async fn status(&self) -> Result<Value> {
        Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"storage":self.database.status().await?,
            "capabilities":{"material":true,"observation":true,"memory_formation":true,"structured_cognition":true,"structured_formation":true,"lexical_query":true,"text_embedding":true,"dense_query":true,"vcp_query":true},
            "model":self.model.inspect(&ConfigScope::Instance).await?,"platform":self.platform.status()?,"index":self.retrieval.coverage().ok()}),
        )
    }

    pub async fn create_namespace(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        name: &str,
    ) -> Result<OperationReceipt> {
        ctx.require_admin()?;
        validate_name(name, "namespace name")?;
        let key = operation_key(
            ctx,
            "namespace.create",
            "instance",
            operation_id,
            &json!({"name":name}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let namespace = Namespace {
            id: NamespaceId::new(),
            name: name.into(),
            revision: 1,
            commit_seq: tx.commit_seq(),
        };
        tx.insert_namespace(&namespace).await?;
        tx.commit(serde_json::to_value(namespace)?).await
    }

    pub async fn list_namespaces(&self, ctx: &CallContext) -> Result<Vec<Namespace>> {
        ctx.require_admin()?;
        self.database.list_namespaces().await
    }

    pub async fn create_subject(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        namespace_id: &NamespaceId,
        name: &str,
        capabilities: Vec<Capability>,
    ) -> Result<OperationReceipt> {
        ctx.require_namespace(namespace_id)?;
        validate_name(name, "subject name")?;
        if capabilities.is_empty() {
            return Err(Error::invalid("at least one capability must be supplied"));
        }
        if capabilities.len() != 1 {
            return Err(Error::invalid(
                "capabilities must be unique; memory is currently supported",
            ));
        }
        self.database.namespace(namespace_id).await?;
        let key = operation_key(
            ctx,
            "subject.create",
            namespace_id.as_str(),
            operation_id,
            &json!({"namespace_id":namespace_id,"name":name,"capabilities":capabilities}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let subject = Subject {
            id: SubjectId::new(),
            namespace_id: namespace_id.clone(),
            name: name.into(),
            capabilities,
            revision: 1,
            commit_seq: tx.commit_seq(),
        };
        tx.namespace(namespace_id).await?;
        tx.insert_subject(&subject).await?;
        tx.commit(serde_json::to_value(subject)?).await
    }

    pub async fn list_subjects(
        &self,
        ctx: &CallContext,
        namespace_id: &NamespaceId,
    ) -> Result<Vec<Subject>> {
        match &ctx.scope {
            AccessScope::Subject {
                namespace_id: allowed,
                subject_id,
            } if allowed == namespace_id => {
                Ok(vec![self.database.subject(namespace_id, subject_id).await?])
            }
            _ => {
                ctx.require_namespace(namespace_id)?;
                self.database.list_subjects(namespace_id).await
            }
        }
    }

    pub async fn put_material(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        namespace_id: &NamespaceId,
        text: &str,
        source: Option<String>,
        event_time: Option<TimePoint>,
    ) -> Result<OperationReceipt> {
        ctx.require_namespace(namespace_id)?;
        if text.is_empty() || text.len() > 8 * 1024 * 1024 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "text must contain 1 byte to 8 MiB",
            ));
        }
        if source.as_ref().is_some_and(|value| value.len() > 4096) {
            return Err(Error::invalid("source is limited to 4096 bytes"));
        }
        if let Some(time) = &event_time {
            time.validate()?;
        }
        self.database.namespace(namespace_id).await?;
        let published = self
            .objects
            .publish_bytes(
                text.as_bytes(),
                8 * 1024 * 1024,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await?;
        let hash = published.digest.clone();
        let key = operation_key(
            ctx,
            "material.put",
            namespace_id.as_str(),
            operation_id,
            &json!({"namespace_id":namespace_id,"content_hash":hash,"media_type":"text/plain; charset=utf-8","source":source,"event_time":event_time}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let material = Material {
            id: MaterialId::new(),
            namespace_id: namespace_id.clone(),
            content_hash: hash,
            media_type: "text/plain; charset=utf-8".into(),
            metadata: nous_domain::media::MediaMetadata::Text,
            byte_len: published.byte_len,
            source,
            event_time,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        published.protect_in(&mut tx);
        tx.namespace(namespace_id).await?;
        tx.insert_material(&material).await?;
        tx.commit(serde_json::to_value(material)?).await
    }

    pub async fn get_material(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
    ) -> Result<MaterialContent> {
        self.sources.original_text(ctx,id,&tokio_util::sync::CancellationToken::new()).await
    }

    pub async fn record_observation(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        subject_id: &SubjectId,
        material_id: &MaterialId,
        observed_at: Option<TimePoint>,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.material_namespace(material_id).await?;
        ctx.require_subject(&namespace, subject_id)?;
        // A subject may record repeat contact with already accessible material;
        // supplying a guessed identifier cannot grant new source access.
        if matches!(ctx.scope, AccessScope::Subject { .. })
            && !self
                .database
                .subject_has_material_access(&namespace, subject_id, material_id)
                .await?
        {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "new material exposure requires namespace authority",
            ));
        }
        self.database.subject(&namespace, subject_id).await?;
        self.database.material(&namespace, material_id).await?;
        if let Some(time) = &observed_at {
            time.validate()?;
        }
        let scope = format!("namespace:{namespace}/subject:{subject_id}");
        let key = operation_key(
            ctx,
            "observation.record",
            &scope,
            operation_id,
            &json!({"subject_id":subject_id,"material_id":material_id,"observed_at":observed_at}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let observation = Observation {
            id: ObservationId::new(),
            namespace_id: namespace,
            subject_id: subject_id.clone(),
            material_id: material_id.clone(),
            observed_at,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
            kind: ObservationKind::Contact,
        };
        tx.subject(&observation.namespace_id, subject_id).await?;
        tx.material(&observation.namespace_id, material_id).await?;
        tx.insert_observation(&observation).await?;
        tx.commit(serde_json::to_value(observation)?).await
    }

    pub async fn list_observations(
        &self,
        ctx: &CallContext,
        subject_id: &SubjectId,
    ) -> Result<Vec<Observation>> {
        let namespace = self.database.subject_namespace(subject_id).await?;
        ctx.require_subject(&namespace, subject_id)?;
        Ok(self
            .database
            .list_observations(subject_id)
            .await?
            .into_iter()
            .filter(|event| event.kind == ObservationKind::Contact)
            .collect())
    }

    pub async fn job(&self, ctx: &CallContext, id: &str) -> Result<Value> {
        let job = self.tasks.read(ctx, id).await?;
        let queue = self.database.queue_record(id).await?;
        let model_calls = self.database.model_calls(id).await?;
        Ok(json!({"job":job,"queue":queue,"model_calls":model_calls}))
    }
    pub async fn read_model_call(&self, ctx: &CallContext, id: &ModelCallId) -> Result<ModelCall> {
        self.calls.read(ctx, id).await
    }
    pub async fn cancel_job(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &str,
    ) -> Result<OperationReceipt> {
        self.tasks.cancel(ctx, operation_id, id).await
    }
    pub(crate) async fn worker_transaction(
        &self,
        job: &JobRecord,
        kind: &str,
        operation_id: &str,
        scope: &str,
        payload: &Value,
    ) -> Result<Option<nous_storage::WriteTransaction>> {
        let context = job.call_context("nous-worker")?;
        let key = operation_key(&context, kind, scope, operation_id, payload)?;
        match self.database.begin_operation(key).await? {
            BeginOperation::Replay(_) => Ok(None),
            BeginOperation::New(tx) => Ok(Some(tx)),
        }
    }
}
