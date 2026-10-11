#![allow(dead_code)]

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

pub(crate) mod longitudinal;
pub(crate) mod query;

use chrono::Utc;
use nous_core::{EpistemicClass, OperationId, TemporalExtent};
use nous_kernel::{NousRuntime, RuntimeOptions};
use nous_material::{
    ObservationInput, ObservationMaterial, OccurrenceDescriptor, RuntimeDirective,
};
use nous_memory::{
    BasisRole, CognitiveRole, EvidenceLocator, EvidenceRef, ExplicitMemoryInput, FormationMode,
    RevisionBasis,
};
use nous_retrieval::ServingOptions;
use postgresql_embedded::{PostgreSQL, SettingsBuilder, VersionReq};
use std::time::Duration;
use tempfile::TempDir;

pub(crate) async fn database() -> (TempDir, String, PostgreSQL) {
    let temporary =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/temp/tests");
    std::fs::create_dir_all(&temporary).expect("test temporary directory");
    let root = TempDir::new_in(temporary).expect("temporary PostgreSQL root");
    let settings = SettingsBuilder::new()
        .version(VersionReq::parse("=18.6.0").expect("version"))
        .trust_installation_dir(true)
        .host("127.0.0.1")
        .port(0)
        .username("postgres")
        .password("nous_wave")
        .installation_dir(
            std::env::var_os("NOUS_WAVE_POSTGRES_RUNTIME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../data/runtime/installed/postgresql")
                }),
        )
        .data_dir(root.path().join("data"))
        .password_file(root.path().join("postgres.pgpass"))
        .timeout(Some(Duration::from_secs(30)))
        .temporary(true)
        .build();
    let mut postgres = PostgreSQL::new(settings);
    let binary = postgres.settings().installation_dir.join(if cfg!(windows) {
        "bin/postgres.exe"
    } else {
        "bin/postgres"
    });
    assert!(
        binary.is_file(),
        "PostgreSQL runtime missing; run just dev-prepare"
    );
    postgres
        .setup()
        .await
        .expect("initialize PostgreSQL cluster");
    postgres.start().await.expect("start PostgreSQL");
    postgres
        .create_database("nous_reference_profile")
        .await
        .expect("create database");
    let url = postgres.settings().url("nous_reference_profile");
    // Return the root first so test bindings drop PostgreSQL before TempDir.
    // PostgreSQL's Drop implementation stops the server; only then can TempDir
    // remove the installation and data tree without leaving a recent .tmp root.
    (root, url, postgres)
}

pub(crate) async fn open_runtime(url: &str, root: &TempDir) -> NousRuntime {
    open_runtime_with_serving(url, root, false, false, false).await
}

pub(crate) async fn open_memory_only_runtime(url: &str, root: &TempDir) -> NousRuntime {
    NousRuntime::open(RuntimeOptions {
        postgres_url: url.into(),
        max_connections: 4,
        acquire_timeout_ms: 15000,
        object_root: root.path().join("objects").to_string_lossy().into_owned(),
        serving_options: ServingOptions {
            root: root.path().join("serving").to_path_buf(),
            lexical: false,
            dense: false,
            topology: false,
            memory_enabled: true,
        },
        embedding: None,
        stored_embedding: None,
        core_descriptors: vec![],
        deployment_document: serde_json::json!({
                "capabilities": {
                    "process": { "memory": true },
                    "subject_defaults": { "memory": true }
                },
                "serving": {
                    "lexical": { "enabled": false },
                    "dense": { "enabled": false },
                    "topology": { "enabled": false }
                }
        }),
    })
    .await
    .expect("open memory-only runtime")
}

pub(crate) async fn open_runtime_with_serving(
    url: &str,
    root: &TempDir,
    lexical: bool,
    dense: bool,
    topology: bool,
) -> NousRuntime {
    NousRuntime::open(RuntimeOptions {
        postgres_url: url.into(),
        max_connections: 4,
        acquire_timeout_ms: 15000,
        object_root: root.path().join("objects").to_string_lossy().into_owned(),
        serving_options: ServingOptions {
            root: root.path().join("serving"),
            lexical,
            dense,
            topology,
            memory_enabled: true,
        },
        embedding: None,
        stored_embedding: None,
        core_descriptors: vec![],
        deployment_document: serde_json::json!({
                "capabilities": {
                    "process": { "memory": true },
                    "subject_defaults": { "memory": true }
                },
                "serving": {
                    "lexical": { "enabled": lexical },
                    "dense": { "enabled": dense },
                    "topology": { "enabled": topology }
                }
        }),
    })
    .await
    .expect("open runtime")
}

pub(crate) async fn observation(
    runtime: &NousRuntime,
    subject: nous_core::SubjectId,
    text: &str,
) -> nous_material::AcceptedObservation {
    runtime
        .material
        .record_observation(ObservationInput {
            subject,
            session: None,
            occurrence: OccurrenceDescriptor {
                source_class: nous_core::SourceClass::Message,
                external_object_ref: None,
                occurred_time: TemporalExtent::Unknown,
                observed_at: Some(Utc::now()),
                conversation_ref: None,
                actor_entity_ref: None,
                context: serde_json::json!({}),
            },
            material: ObservationMaterial::InlineText {
                text: text.into(),
                media_type: "text/plain".into(),
            },
            entities: Vec::new(),
            runtime: RuntimeDirective::default(),
        })
        .await
        .expect("record observation")
}

pub(crate) async fn external_observation(
    runtime: &NousRuntime,
    subject: nous_core::SubjectId,
    object_ref: &str,
) -> nous_material::AcceptedObservation {
    let object_ref = nous_core::ObjectRef::new(object_ref).expect("object ref");
    runtime
        .material
        .record_observation(ObservationInput {
            subject,
            session: None,
            occurrence: OccurrenceDescriptor {
                source_class: nous_core::SourceClass::Message,
                external_object_ref: Some(object_ref.clone()),
                occurred_time: TemporalExtent::Unknown,
                observed_at: Some(Utc::now()),
                conversation_ref: None,
                actor_entity_ref: None,
                context: serde_json::json!({}),
            },
            material: ObservationMaterial::ExternalObjectRef { object_ref },
            entities: Vec::new(),
            runtime: RuntimeDirective {
                admit: false,
                hold_until: None,
            },
        })
        .await
        .expect("record external observation")
}

pub(crate) async fn external_actor_observation(
    runtime: &NousRuntime,
    subject: nous_core::SubjectId,
    actor: &str,
    object_ref: &str,
) -> nous_material::AcceptedObservation {
    let actor = nous_core::EntityRef::new(actor).expect("actor ref");
    let object_ref = nous_core::ObjectRef::new(object_ref).expect("object ref");
    runtime
        .material
        .record_observation(ObservationInput {
            subject,
            session: None,
            occurrence: OccurrenceDescriptor {
                source_class: nous_core::SourceClass::Message,
                external_object_ref: Some(object_ref.clone()),
                occurred_time: TemporalExtent::Unknown,
                observed_at: Some(Utc::now()),
                conversation_ref: None,
                actor_entity_ref: Some(actor),
                context: serde_json::json!({}),
            },
            material: ObservationMaterial::ExternalObjectRef { object_ref },
            entities: Vec::new(),
            runtime: RuntimeDirective {
                admit: false,
                hold_until: None,
            },
        })
        .await
        .expect("record external actor observation")
}

pub(crate) async fn occurrence_only_observation(
    runtime: &NousRuntime,
    subject: nous_core::SubjectId,
) -> nous_material::AcceptedObservation {
    runtime
        .material
        .record_observation(ObservationInput {
            subject,
            session: None,
            occurrence: OccurrenceDescriptor {
                source_class: nous_core::SourceClass::HostEvent,
                external_object_ref: None,
                occurred_time: TemporalExtent::Unknown,
                observed_at: Some(Utc::now()),
                conversation_ref: None,
                actor_entity_ref: None,
                context: serde_json::json!({}),
            },
            material: ObservationMaterial::ResourceAvailability {
                resource: nous_core::ResourceRef::new("resource:unknown").expect("resource ref"),
            },
            entities: Vec::new(),
            runtime: RuntimeDirective {
                admit: false,
                hold_until: None,
            },
        })
        .await
        .expect("record occurrence-only observation")
}

pub(crate) fn form_input(
    subject: nous_core::SubjectId,
    occurrence: nous_core::OccurrenceId,
    operation_id: OperationId,
    text: &str,
) -> ExplicitMemoryInput {
    ExplicitMemoryInput {
        producer: None,
        operation_id,
        subject,
        cognitive_role: CognitiveRole::Declarative,
        formation_mode: FormationMode::Grounded,
        grounding_occurrence_id: Some(occurrence),
        semantic_role: "fact".into(),
        representation_text: text.into(),
        title: None,
        basis: vec![RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation: None,
            occurrence_id: occurrence,
            locator: EvidenceLocator::WholeOccurrence,
            basis_role: BasisRole::Direct,
        })],
        aboutness: Vec::new(),
        tags: Vec::new(),
        valid_time: TemporalExtent::Unknown,

        epistemic_class: EpistemicClass::Observed,
    }
}

pub(crate) struct LongitudinalEmbedding;

#[async_trait::async_trait]
impl nous_retrieval::TextEmbeddingProvider for LongitudinalEmbedding {
    fn space(&self) -> nous_core::EmbeddingSpaceSignature {
        nous_core::EmbeddingSpaceSignature {
            space_hash: "longitudinal-test-space".into(),
            model_identity: "deterministic-test".into(),
            weights_revision: "1".into(),
            task: "text".into(),
            input_representation: "text".into(),
            preprocessing_identity: "identity".into(),
            preprocessing_revision: "1".into(),
            dimension: 2,
            normalization: "l2".into(),
            output_semantics: "test-vector".into(),
        }
    }
    fn producer(&self) -> nous_core::ProducerSignature {
        nous_core::ProducerSignature {
            model_role: None,
            model_profile: None,
            execution_profile: None,
            inference_controls_digest: None,
            role_policy_digest: None,
            prompt_id: None,
            prompt_digest: None,

            signature_hash: "longitudinal-test-producer".into(),
            provider_class: "test".into(),
            operation: nous_core::CapabilityOperation::TextEmbedding,
            implementation: "test".into(),
            model_identity: Some("deterministic-test".into()),
            model_revision: Some("1".into()),
            output_schema_digest: None,
            preprocessing_identity: "identity".into(),
            preprocessing_revision: "1".into(),
            config_digest: "test".into(),
        }
    }
    async fn embed(
        &self,
        _request: nous_retrieval::TextEmbeddingRequest,
    ) -> nous_core::Result<nous_retrieval::TextEmbeddingOutput> {
        Ok(nous_retrieval::TextEmbeddingOutput {
            vector: vec![1.0, 0.0],
            space: self.space(),
            producer: self.producer(),
        })
    }
}

pub(crate) fn query_lease() -> nous_runtime::QueryLease {
    nous_runtime::QueryLease::new(Duration::from_secs(30)).unwrap()
}
pub(crate) fn query_reservation(bound: nous_runtime::BoundQuery) -> nous_runtime::QueryReservation {
    nous_runtime::QueryReservation::new(bound, query_lease()).unwrap()
}

#[allow(dead_code)]
pub(crate) fn workflow_payload(payload: serde_json::Value) -> nous_core::WorkflowPayload {
    nous_core::WorkflowPayload {
        payload,
        ..Default::default()
    }
}
#[allow(dead_code)]
pub(crate) fn workflow_snapshot(payload: serde_json::Value) -> nous_core::WorkflowSnapshot {
    nous_core::WorkflowSnapshot {
        content: workflow_payload(payload),
        ..Default::default()
    }
}

#[allow(dead_code)]
pub(crate) fn workflow_lease(
    subject: nous_core::SubjectId,
    owner: &str,
    key: &str,
    token: uuid::Uuid,
) -> nous_persistence::WorkflowLease {
    nous_persistence::WorkflowLease {
        subject,
        owner: nous_persistence::WorkflowOwner::new(owner).unwrap(),
        operation_key: key.into(),
        token,
    }
}
