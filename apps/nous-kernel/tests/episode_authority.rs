// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_kernel::NousRuntime;
use nous_material::ObservationMaterial;
use nous_memory::{
    EpisodePartitionInput, EpisodePartitionSegment, EpisodePartitionSource, EpisodeView,
};
use nous_runtime::{CognitiveClock, MaintenanceDisposition, ManualCognitiveClock};
use std::sync::Arc;
use test_support::database;

use test_support::longitudinal::{observation, runtime_with_clock};
use test_support::query::subject as create_subject;

async fn partition_request(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    sources: &[EpisodeView],
    occurrences: &[nous_core::OccurrenceId],
    groups: &[Vec<usize>],
) -> EpisodePartitionInput {
    EpisodePartitionInput {
        operation_id: nous_core::OperationId::new(),
        subject,
        expected_authority_seq: rt.store.authority_seq(subject).await.unwrap(),
        sources: sources
            .iter()
            .map(|source| EpisodePartitionSource {
                revision: source.revision.episode_revision_id,
                expected_epoch: source.object.object_epoch,
            })
            .collect(),
        ordered_occurrences: occurrences.to_vec(),
        segments: groups
            .iter()
            .enumerate()
            .map(|(index, indices)| EpisodePartitionSegment {
                member_indices: indices.clone(),
                title: Some(format!("segment {index}")),
                boundary_explanation: "supported local organization".into(),
            })
            .collect(),
        producer: None,
    }
}

#[tokio::test]
async fn episode_partition_split_repartition_merge_and_retry_are_atomic() {
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    let mut occurrences = vec![];
    for index in 0..4 {
        let mut input = observation(subject, Some(session.session_id));
        input.material = ObservationMaterial::InlineText {
            text: format!("experience {index}"),
            media_type: "text/plain".into(),
        };
        occurrences.push(
            rt.material
                .record_observation(input)
                .await
                .unwrap()
                .occurrence
                .occurrence_id,
        );
        clock.advance_by(subject, Duration::minutes(1)).unwrap();
    }
    let original = rt
        .organize_experience(subject, 128, true)
        .await
        .unwrap()
        .episodes;
    let memory = rt.require_memory().unwrap();
    let split_request = partition_request(
        &rt,
        subject,
        &original,
        &occurrences,
        &[vec![0, 1], vec![2, 3]],
    )
    .await;
    let split = memory
        .apply_episode_partition(split_request.clone())
        .await
        .unwrap();
    assert_eq!(split.len(), 2);
    assert!(
        split
            .iter()
            .all(|part| part.object.episode_id != original[0].object.episode_id)
    );
    let withdrawn = memory
        .episode(subject, original[0].object.episode_id, None)
        .await
        .unwrap();
    assert_eq!(
        withdrawn.object.acceptance_state,
        nous_memory::AcceptanceState::Withdrawn
    );
    assert!(
        memory
            .reaccept_episode(
                subject,
                withdrawn.object.episode_id,
                nous_core::OperationId::new(),
                withdrawn.object.object_epoch
            )
            .await
            .is_err()
    );
    assert!(split.iter().all(|part| {
        part.relations
            .iter()
            .any(|relation| relation.relation == "split_from")
    }));
    let replay = memory.apply_episode_partition(split_request).await.unwrap();
    assert_eq!(
        replay[0].revision.episode_revision_id,
        split[0].revision.episode_revision_id
    );
    assert_partition_rollback(&rt, subject, &split, &occurrences).await;
    let repartition_request = partition_request(
        &rt,
        subject,
        &split,
        &occurrences,
        &[vec![0], vec![1, 2, 3]],
    )
    .await;
    let repartition = memory
        .apply_episode_partition(repartition_request)
        .await
        .unwrap();
    assert!(repartition.iter().all(|part| {
        part.relations
            .iter()
            .filter(|relation| relation.relation == "derived_from")
            .count()
            == 2
    }));
    let merge_request = partition_request(
        &rt,
        subject,
        &repartition,
        &occurrences,
        &[vec![0, 1, 2, 3]],
    )
    .await;
    let merged = memory.apply_episode_partition(merge_request).await.unwrap();
    assert_eq!(merged.len(), 1);
    assert!(
        merged[0]
            .relations
            .iter()
            .filter(|relation| relation.relation == "merged_from")
            .count()
            == 2
    );
    assert_partition_revision(&rt, subject, &merged, &occurrences).await;
}

async fn assert_partition_rollback(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    sources: &[EpisodeView],
    occurrences: &[nous_core::OccurrenceId],
) {
    let mut invalid =
        partition_request(rt, subject, sources, occurrences, &[vec![0, 1], vec![2, 3]]).await;
    invalid.segments[1].boundary_explanation = "x".repeat(20000);
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM episode_objects WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(rt.store.pool())
            .await
            .unwrap();
    let watermark = rt.store.authority_seq(subject).await.unwrap();
    assert!(
        rt.require_memory()
            .unwrap()
            .apply_episode_partition(invalid)
            .await
            .is_err()
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM episode_objects WHERE subject_id=$1")
        .bind(subject.0)
        .fetch_one(rt.store.pool())
        .await
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(watermark, rt.store.authority_seq(subject).await.unwrap());
    for source in sources {
        let current = rt
            .require_memory()
            .unwrap()
            .episode(subject, source.object.episode_id, None)
            .await
            .unwrap();
        assert_eq!(
            current.object.acceptance_state,
            nous_memory::AcceptanceState::Accepted
        );
        assert_eq!(
            current.object.current_revision_id,
            source.object.current_revision_id
        );
    }
}

async fn assert_partition_revision(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    merged: &[EpisodeView],
    occurrences: &[nous_core::OccurrenceId],
) {
    let memory = rt.require_memory().unwrap();
    let mut revise = partition_request(rt, subject, merged, occurrences, &[vec![0, 1, 2, 3]]).await;
    revise.segments[0].title = Some("reinterpreted scope".into());
    let revised = memory
        .apply_episode_partition(revise.clone())
        .await
        .unwrap();
    assert_eq!(revised[0].object.episode_id, merged[0].object.episode_id);
    assert_ne!(
        revised[0].revision.episode_revision_id,
        merged[0].revision.episode_revision_id
    );
    assert_eq!(
        memory
            .episode_history(subject, revised[0].object.episode_id)
            .await
            .unwrap()
            .len(),
        2
    );
    let mut unchanged =
        partition_request(rt, subject, &revised, occurrences, &[vec![0, 1, 2, 3]]).await;
    unchanged.segments[0].title = revised[0].revision.title.clone();
    let watermark = rt.store.authority_seq(subject).await.unwrap();
    let no_change = memory.apply_episode_partition(unchanged).await.unwrap();
    assert_eq!(
        no_change[0].revision.episode_revision_id,
        revised[0].revision.episode_revision_id
    );
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), watermark);
    revise.operation_id = nous_core::OperationId::new();
    assert!(memory.apply_episode_partition(revise).await.is_err());
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one regression spans capture, planning and atomic repair of historical chronology"
)]
async fn late_experience_uses_chronology_and_repairs_old_local_partitions() {
    use nous_core::CognitiveRef;
    use nous_protocol::{
        kernel as k, kernel::kernel_maintenance_service_server::KernelMaintenanceService,
    };
    let (root, url, _postgres) = database().await;
    let historical = chrono::DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let clock = Arc::new(ManualCognitiveClock::new(historical + Duration::days(730)));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    assert_eq!(
        rt.subjects.subject(subject).await.unwrap().created_at,
        clock.now(subject)
    );
    let session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    let mut occurrences = Vec::new();
    for (label, offset) in [("A", 0), ("C", 20)] {
        let mut input = observation(subject, Some(session.session_id));
        input.occurrence.observed_at = Some(historical + Duration::minutes(offset));
        input.material = ObservationMaterial::InlineText {
            text: label.into(),
            media_type: "text/plain".into(),
        };
        occurrences.push(
            rt.material
                .record_observation(input)
                .await
                .unwrap()
                .occurrence
                .occurrence_id,
        );
    }
    let original = rt
        .organize_experience(subject, 256, true)
        .await
        .unwrap()
        .episodes;
    assert_eq!(original.len(), 1);
    let mut input = observation(subject, Some(session.session_id));
    input.occurrence.observed_at = Some(historical + Duration::minutes(10));
    input.material = ObservationMaterial::InlineText {
        text: "B".into(),
        media_type: "text/plain".into(),
    };
    let late = rt
        .material
        .record_observation(input)
        .await
        .unwrap()
        .occurrence
        .occurrence_id;
    assert!(
        rt.organize_experience(subject, 256, true)
            .await
            .unwrap()
            .episodes
            .is_empty()
    );
    let service = nous_kernel::transport::KernelService(rt.clone());
    let kinds = vec!["episode_resegment".into()];
    let needs = rt
        .cognition
        .lease_maintenance(subject, &kinds, 8, 60, None)
        .await
        .unwrap();
    let need = needs
        .iter()
        .find(|need| need.scope_kind == "track")
        .unwrap();
    let plan = service
        .plan_maintenance(tonic::Request::new(k::PlanMaintenanceRequest {
            claimed: Some(k::MaintenanceNeed {
                need_id: need.need_id.to_string(),
                subject_id: subject.0.to_string(),
                kind: need.kind.clone(),
                scope_kind: need.scope_kind.clone(),
                scope_ref: need.scope_ref.clone(),
                trigger_authority_seq: need.trigger_authority_seq,
                due_at: Some(test_timestamp(need.due_at)),
                lease_token: need.lease_token.map(|id| id.to_string()),
                created_at: Some(test_timestamp(need.created_at)),
                updated_at: Some(test_timestamp(need.updated_at)),
                ..Default::default()
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(plan.status, "ready");
    let chronology = vec![occurrences[0], late, occurrences[1]];
    assert_eq!(
        plan.members
            .iter()
            .map(|member| member.occurrence_id.clone())
            .collect::<Vec<_>>(),
        chronology
            .iter()
            .map(|id| id.0.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        plan.members
            .iter()
            .map(|member| member.text.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B", "C"]
    );
    assert!(plan.members[1].recorded_seq > plan.members[2].recorded_seq);
    let partition = partition_request(&rt, subject, &original, &chronology, &[vec![0, 1, 2]]).await;
    let committed = rt
        .require_memory()
        .unwrap()
        .apply_episode_partition(partition)
        .await
        .unwrap();
    assert_eq!(
        committed[0]
            .members
            .iter()
            .map(|member| member.reference.clone())
            .collect::<Vec<_>>(),
        chronology
            .into_iter()
            .map(CognitiveRef::Occurrence)
            .collect::<Vec<_>>()
    );
    rt.cognition
        .acknowledge_maintenance(need, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
}

fn test_timestamp(at: chrono::DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: at.timestamp(),
        nanos: at.timestamp_subsec_nanos() as i32,
    }
}
