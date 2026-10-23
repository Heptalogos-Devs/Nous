pub mod backup;
pub mod bundle_format;
mod cognition;
pub mod import_format;
mod imports;
pub mod instance;
mod maintenance;
pub mod model;
pub mod network;
mod objects;
mod worker;

use chrono::Utc;
use instance::InstancePaths;
use model::ModelRuntime;
use nous_domain::*;
use nous_retrieval::RetrievalIndex;
pub use nous_storage::OperationReceipt;
use nous_storage::{BeginOperation, Database, OperationKey};
use objects::ObjectStore;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// The shared entrypoint used by every transport. Domain rules live here;
/// durable commit and queue mechanics live in nous-storage.
#[derive(Clone)]
pub struct App {
    database: Database,
    objects: ObjectStore,
    paths: Arc<InstancePaths>,
    model: Arc<ModelRuntime>,
    retrieval: RetrievalIndex,
    index_gate: Arc<tokio::sync::Mutex<()>>,
}

impl App {
    pub async fn open(paths: InstancePaths) -> Result<Self> {
        let paths = Arc::new(paths);
        let database = Database::open(&paths.database).await?;
        let objects = ObjectStore::new(paths.clone());
        let config = if paths.config.exists() {
            instance::InstanceConfig::load(&paths)
                .map_err(|error| Error::invalid(error.to_string()))?
        } else {
            instance::InstanceConfig::default()
        };
        let network = network::NetworkClients::new(&config.network)
            .map_err(|error| Error::new(ErrorCode::DependencyUnavailable, error.to_string()))?;
        let model = Arc::new(
            ModelRuntime::new(&paths, &config.models, network.http().clone())
                .map_err(|error| Error::invalid(error.to_string()))?,
        );
        let index_path = paths
            .resolve("indexes/lexical-v1")
            .map_err(|error| Error::invalid(error.to_string()))?;
        let retrieval = tokio::task::spawn_blocking(move || RetrievalIndex::open(index_path))
            .await
            .map_err(|error| Error::internal(error.to_string()))?
            .map_err(|error| Error::new(ErrorCode::DependencyUnavailable, error.to_string()))?;
        Ok(Self {
            database,
            objects,
            paths,
            model,
            retrieval,
            index_gate: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub fn paths(&self) -> &InstancePaths {
        &self.paths
    }
    pub async fn close(&self) {
        self.database.close().await;
        let index = self.retrieval.clone();
        match tokio::task::spawn_blocking(move || index.close()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error,"index close failed"),
            Err(error) => tracing::warn!(%error,"index close task failed"),
        }
    }

    pub async fn status(&self) -> Result<Value> {
        Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"storage":self.database.status().await?,
            "capabilities":{"material":true,"observation":true,"memory_formation":true,"lexical_query":true},
            "model":self.model.inspect(),"index":self.retrieval.coverage().ok()}),
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
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        let key = operation_key(
            ctx,
            "material.put",
            namespace_id.as_str(),
            operation_id,
            &json!({"namespace_id":namespace_id,"content_hash":hash,"media_type":"text/plain; charset=utf-8","source":source,"event_time":event_time}),
        )?;
        // Publishing and verifying bytes must not hold SQLite's single writer.
        let object_store = self.objects.clone();
        let bytes = text.as_bytes().to_vec();
        tokio::task::spawn_blocking(move || object_store.publish(&bytes))
            .await
            .map_err(|error| Error::internal(error.to_string()))??;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let material = Material {
            id: MaterialId::new(),
            namespace_id: namespace_id.clone(),
            content_hash: hash,
            media_type: "text/plain; charset=utf-8".into(),
            byte_len: text.len() as i64,
            source,
            event_time,
            recorded_at: Utc::now().to_rfc3339(),
            commit_seq: tx.commit_seq(),
        };
        tx.namespace(namespace_id).await?;
        tx.insert_material(&material).await?;
        tx.commit(serde_json::to_value(material)?).await
    }

    pub async fn get_material(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
    ) -> Result<MaterialContent> {
        let namespace = self.database.material_namespace(id).await?;
        if let AccessScope::Subject {
            namespace_id,
            subject_id,
        } = &ctx.scope
        {
            ctx.require_subject(&namespace, subject_id)?;
            if !self
                .database
                .subject_has_material_access(namespace_id, subject_id, id)
                .await?
            {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "material access has not been granted to this subject",
                ));
            }
        } else {
            ctx.require_namespace(&namespace)?;
        }
        let material = self.database.material(&namespace, id).await?;
        let store = self.objects.clone();
        let digest = material.content_hash.clone();
        let byte_len = material.byte_len;
        let bytes = tokio::task::spawn_blocking(move || store.read(&digest, byte_len))
            .await
            .map_err(|error| Error::internal(error.to_string()))??;
        let text = String::from_utf8(bytes)
            .map_err(|_| Error::internal("text object contains invalid UTF-8"))?;
        let details = self.database.material_details(&namespace, id).await?;
        Ok(MaterialContent {
            material,
            text,
            details,
        })
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
            recorded_at: Utc::now().to_rfc3339(),
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
        let job = self.database.job(id).await?;
        let namespace: NamespaceId = job.envelope.namespace_id.parse()?;
        if let Some(subject) = &job.envelope.subject_id {
            ctx.require_subject(&namespace, &subject.parse()?)?;
        } else {
            ctx.require_namespace(&namespace)?;
        }
        let mut value = serde_json::to_value(&job)?;
        if job.kind == "memory.form" {
            let run = self.database.formation(id).await?;
            value["queue_status"] = value["status"].clone();
            value["status"] = serde_json::to_value(&run.state)?;
            value["formation"] = serde_json::to_value(run)?;
            value["model_calls"] = serde_json::to_value(self.database.model_calls(id).await?)?;
        }
        Ok(value)
    }
}

fn operation_key(
    ctx: &CallContext,
    operation: &str,
    scope: &str,
    operation_id: &str,
    payload: &impl Serialize,
) -> Result<OperationKey> {
    validate_name(operation_id, "operation_id")?;
    validate_name(&ctx.caller_id, "caller_id")?;
    let encoded = serde_json::to_vec(
        &json!({"schema":"nous.operation.v1","operation":operation,"scope":scope,"payload":payload}),
    )?;
    Ok(OperationKey {
        caller_id: ctx.caller_id.clone(),
        operation: operation.into(),
        scope: scope.into(),
        operation_id: operation_id.into(),
        request_hash: format!("{:x}", Sha256::digest(encoded)),
    })
}
