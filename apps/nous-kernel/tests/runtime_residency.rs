// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;

use chrono::Utc;
use nous_core::{CognitiveRef, EntityRef};
use nous_material::{
    ObservationInput, ObservationMaterial, OccurrenceDescriptor, ResolvedEntityMention,
    RuntimeDirective,
};
use test_support::{database, open_runtime};
use uuid::Uuid;

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one admission journey checks transaction, observation identity and runtime batch replay together"
)]
async fn observation_admission_is_a_single_idempotent_runtime_batch() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let session = runtime
        .cognition
        .open_session(subject, None, serde_json::json!({"consumer":"a"}))
        .await
        .expect("session");
    let entity = EntityRef::new("entity:alice").expect("entity");
    let request_id = Uuid::new_v4();
    let input = ObservationInput {
        subject,
        session: Some(session.session_id),
        occurrence: OccurrenceDescriptor {
            occurred_time_domain_ref: "time:real".into(),
            source_class: nous_core::SourceClass::Message,
            world_realized: None,
            external_object_ref: None,
            occurred_time: nous_core::TemporalExtent::Unknown,
            observed_at: Some(Utc::now()),
            observed_stamp: None,
            conversation_ref: None,
            actor_entity_ref: None,
            context: serde_json::json!({}),
        },
        material: ObservationMaterial::InlineText {
            text: "Alice was present".into(),
            media_type: "text/plain".into(),
        },
        entities: vec![ResolvedEntityMention {
            surface: "Alice".into(),
            entity_ref: Some(entity.clone()),
            semantic_role: Some("subject".into()),
        }],
        runtime: RuntimeDirective::default(),
    };
    let first = runtime
        .material
        .record_observation_once(input.clone(), Some(request_id))
        .await
        .expect("first observation");
    let after_first = runtime
        .cognition
        .session(subject, session.session_id)
        .await
        .expect("resident session");
    assert_eq!(after_first.runtime_revision, 1);
    assert_eq!(after_first.resident.len(), 2);
    assert!(
        after_first.resident.iter().any(
            |value| value.reference == CognitiveRef::Occurrence(first.occurrence.occurrence_id)
        )
    );
    assert!(
        after_first
            .resident
            .iter()
            .any(|value| value.reference == CognitiveRef::Entity(entity.clone()))
    );

    let retry = runtime
        .material
        .record_observation_once(input.clone(), Some(request_id))
        .await
        .expect("idempotent observation retry");
    let after_retry = runtime
        .cognition
        .session(subject, session.session_id)
        .await
        .expect("resident retry session");
    assert_eq!(
        retry.occurrence.occurrence_id,
        first.occurrence.occurrence_id
    );
    assert_eq!(after_retry.runtime_revision, after_first.runtime_revision);
    assert_eq!(after_retry.resident.len(), after_first.resident.len());
    let mut rejected = input.clone();
    rejected.entities.clear();
    rejected.material = ObservationMaterial::ResourceAvailability {
        resource: nous_core::ResourceRef::new("resource:test:missing").unwrap(),
    };
    let rejected_key = Uuid::new_v4();
    assert!(matches!(
        runtime
            .material
            .record_observation_once(rejected, Some(rejected_key))
            .await,
        Err(nous_core::Error::FailedPrecondition(_))
    ));
    let occurrences: i64 =
        sqlx::query_scalar("SELECT count(*) FROM observation_occurrences WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(runtime.store.pool())
            .await
            .unwrap();
    assert_eq!(
        occurrences, 1,
        "failed Runtime admission must roll back Material Authority"
    );
    let closed = runtime
        .cognition
        .close_session(subject, session.session_id)
        .await
        .unwrap();
    let replay = runtime
        .material
        .record_observation_once(input, Some(request_id))
        .await
        .unwrap();
    assert_eq!(
        replay.occurrence.occurrence_id,
        first.occurrence.occurrence_id
    );
    let after_closed_replay = runtime
        .cognition
        .session(subject, session.session_id)
        .await
        .unwrap();
    assert_eq!(
        after_closed_replay.runtime_revision,
        closed.runtime_revision
    );
}
