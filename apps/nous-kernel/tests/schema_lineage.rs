// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{SubsecRound, Utc};
use nous_core::OperationId;
use nous_memory::AcceptanceState;
use nous_runtime::ManualCognitiveClock;
use std::sync::Arc;
use test_support::database;
use test_support::longitudinal::{
    consolidation_schema, journal_source_episode, runtime_with_clock,
};
use test_support::query::subject as create_subject;

#[tokio::test]
async fn split_and_merge_share_content_writer_without_partial_authority() {
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let episode = journal_source_episode(&rt, &clock, subject).await;
    let owner = rt.require_memory().unwrap();
    let source = owner
        .create_schema(consolidation_schema(&episode))
        .await
        .unwrap();
    let split = assert_split(&rt, subject, &source, &episode).await;
    assert_merge(&rt, subject, &source, &split, &episode).await;
}

async fn assert_split(
    rt: &nous_kernel::NousRuntime,
    subject: nous_core::SubjectId,
    source: &nous_memory::schema::SchemaView,
    episode: &nous_memory::EpisodeView,
) -> Vec<nous_memory::schema::SchemaView> {
    let owner = rt.require_memory().unwrap();
    let mut children = vec![consolidation_schema(episode), consolidation_schema(episode)];
    children[0].content.structural_claim = "First scope of the recurring pattern".into();
    children[1].content.structural_claim = "Second scope of the recurring pattern".into();
    let mut invalid = children.clone();
    invalid[1].content.evidence_links.clear();
    assert!(
        owner
            .split_schemas(
                subject,
                source.schema.schema_id,
                OperationId::new(),
                source.schema.object_epoch,
                invalid
            )
            .await
            .is_err()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cognitive_schemas WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(rt.store.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        owner
            .schema(subject, source.schema.schema_id)
            .await
            .unwrap()
            .schema
            .object_epoch,
        source.schema.object_epoch
    );

    let operation = OperationId::new();
    let split = owner
        .split_schemas(
            subject,
            source.schema.schema_id,
            operation,
            source.schema.object_epoch,
            children.clone(),
        )
        .await
        .unwrap();
    let replay = owner
        .split_schemas(
            subject,
            source.schema.schema_id,
            operation,
            source.schema.object_epoch,
            children.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|child| child.revision.schema_revision_id)
            .collect::<Vec<_>>(),
        split
            .iter()
            .map(|child| child.revision.schema_revision_id)
            .collect::<Vec<_>>()
    );
    // An old committed split had no result list. Its single lineage batch is recoverable.
    sqlx::query(
        "UPDATE mutation_receipts SET result_ref=$3 WHERE subject_id=$1 AND operation_id=$2",
    )
    .bind(subject.0)
    .bind(operation.0)
    .bind(source.schema.schema_id.0.to_string())
    .execute(rt.store.pool())
    .await
    .unwrap();
    sqlx::raw_sql("ALTER TABLE model_workflow_operations DROP COLUMN dependencies; ALTER TABLE model_workflow_operations DROP COLUMN mutation_operations;").execute(rt.store.pool()).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../crates/persistence/migrations/0005_workflow_envelope.sql"
    ))
    .execute(rt.store.pool())
    .await
    .unwrap();
    let recovered = owner
        .split_schemas(
            subject,
            source.schema.schema_id,
            operation,
            source.schema.object_epoch,
            children,
        )
        .await
        .unwrap();
    assert_eq!(
        recovered
            .iter()
            .map(|child| child.revision.schema_revision_id)
            .collect::<std::collections::BTreeSet<_>>(),
        split
            .iter()
            .map(|child| child.revision.schema_revision_id)
            .collect::<std::collections::BTreeSet<_>>()
    );
    assert_eq!(split.len(), 2);
    for child in &split {
        assert!(child.revision.producer_signature_id.is_some());
        assert_eq!(child.evidence_links.len(), source.evidence_links.len());
        let origins = lineage(rt, child.revision.schema_revision_id, "schema_split_from").await;
        assert_eq!(origins, vec![source.revision.schema_revision_id.0]);
    }
    assert_eq!(
        owner
            .schema(subject, source.schema.schema_id)
            .await
            .unwrap()
            .schema
            .acceptance_state,
        AcceptanceState::Withdrawn
    );
    split
}

async fn assert_merge(
    rt: &nous_kernel::NousRuntime,
    subject: nous_core::SubjectId,
    source: &nous_memory::schema::SchemaView,
    split: &[nous_memory::schema::SchemaView],
    episode: &nous_memory::EpisodeView,
) {
    let owner = rt.require_memory().unwrap();
    let ids: Vec<_> = split.iter().map(|child| child.schema.schema_id).collect();
    let epochs: Vec<_> = split
        .iter()
        .map(|child| child.schema.object_epoch)
        .collect();
    let mut merged_input = consolidation_schema(episode);
    merged_input.content.evidence_links.clear(); // The merge inherits the exact support union.
    let mut stale_epochs = epochs.clone();
    stale_epochs[0] += 1;
    assert!(
        owner
            .merge_schemas(
                subject,
                OperationId::new(),
                ids.clone(),
                stale_epochs,
                merged_input.clone()
            )
            .await
            .is_err()
    );
    for child in split {
        let unchanged = owner.schema(subject, child.schema.schema_id).await.unwrap();
        assert_eq!(unchanged.schema.object_epoch, child.schema.object_epoch);
        assert_eq!(unchanged.schema.acceptance_state, AcceptanceState::Accepted);
    }
    let operation = OperationId::new();
    let merged = owner
        .merge_schemas(
            subject,
            operation,
            ids.clone(),
            epochs.clone(),
            merged_input.clone(),
        )
        .await
        .unwrap();
    let replay = owner
        .merge_schemas(subject, operation, ids, epochs, merged_input)
        .await
        .unwrap();
    assert_eq!(
        replay.revision.schema_revision_id,
        merged.revision.schema_revision_id
    );
    assert!(merged.revision.producer_signature_id.is_some());
    assert_eq!(merged.evidence_links.len(), source.evidence_links.len());
    let origins = lineage(rt, merged.revision.schema_revision_id, "schema_merged_from").await;
    assert_eq!(origins.len(), 2);
    for child in split {
        assert!(origins.contains(&child.revision.schema_revision_id.0));
        assert_eq!(
            owner
                .schema(subject, child.schema.schema_id)
                .await
                .unwrap()
                .schema
                .acceptance_state,
            AcceptanceState::Withdrawn
        );
    }
}

async fn lineage(
    rt: &nous_kernel::NousRuntime,
    revision: nous_core::CognitiveSchemaRevisionId,
    relation: &str,
) -> Vec<uuid::Uuid> {
    sqlx::query_scalar("SELECT to_revision_id FROM cognitive_schema_lineage WHERE from_revision_id=$1 AND relation=$2")
        .bind(revision.0).bind(relation).fetch_all(rt.store.pool()).await.unwrap()
}
