// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{SubsecRound, Utc};
use nous_core::{CognitiveRef, CognitiveSchemaId, OperationId, SubjectId};
use nous_memory::{AcceptanceState, CognitionView, MemoryService};
use nous_runtime::ManualCognitiveClock;
use std::sync::Arc;
use test_support::database;
use test_support::longitudinal::{
    consolidation_schema, journal_source_episode, runtime_with_clock_serving,
};
use test_support::query::subject as create_subject;
use test_support::read_mutation_cognition;

async fn read_schema(
    owner: &MemoryService,
    subject: SubjectId,
    schema_id: CognitiveSchemaId,
) -> nous_memory::schema::SchemaView {
    match owner
        .read_cognition(subject, CognitiveRef::CognitiveSchema(schema_id))
        .await
        .unwrap()
    {
        CognitionView::CognitiveSchema(schema) => schema,
        _ => panic!("common Cognition read returned the wrong view kind"),
    }
}

#[tokio::test]
async fn split_and_merge_share_content_writer_without_partial_authority() {
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock_serving(&url, &root, clock.clone(), false).await;
    let subject = create_subject(&rt).await;
    let episode = journal_source_episode(&rt, &clock, subject).await;
    let owner = rt.require_memory().unwrap();
    let source_result = owner
        .create_schema(consolidation_schema(&episode))
        .await
        .unwrap();
    let source = match read_mutation_cognition(owner, subject, &source_result).await {
        CognitionView::CognitiveSchema(value) => value,
        _ => panic!("schema mutation result should materialize as CognitiveSchema"),
    };
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
                source.header.schema_id(),
                OperationId::new(),
                source.header.object_epoch,
                invalid
            )
            .await
            .is_err()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cognition_objects WHERE subject_id=$1 AND object_kind='cognitive_schema'")
            .bind(subject.0)
            .fetch_one(rt.store.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
    let unchanged = read_schema(owner, subject, source.header.schema_id()).await;
    assert_eq!(unchanged.header.object_epoch, source.header.object_epoch);

    let operation = OperationId::new();
    let split_results = owner
        .split_schemas(
            subject,
            source.header.schema_id(),
            operation,
            source.header.object_epoch,
            children.clone(),
        )
        .await
        .unwrap();
    let replay = owner
        .split_schemas(
            subject,
            source.header.schema_id(),
            operation,
            source.header.object_epoch,
            children.clone(),
        )
        .await
        .unwrap();
    assert_eq!(replay, split_results);
    let child_results = split_results
        .iter()
        .filter(|result| matches!(&result.reference, CognitiveRef::CognitiveSchemaRevision(_)))
        .collect::<Vec<_>>();
    assert_eq!(child_results.len(), 2);
    let mut split = Vec::with_capacity(child_results.len());
    for result in child_results {
        match read_mutation_cognition(owner, subject, result).await {
            CognitionView::CognitiveSchema(value) => split.push(value),
            _ => panic!("split result should materialize as CognitiveSchema"),
        }
    }
    assert_eq!(split.len(), 2);
    for child in &split {
        assert!(child.revision.header.producer_signature_id.is_some());
        assert_eq!(child.evidence_links.len(), source.evidence_links.len());
        let origins = lineage(
            rt,
            child.revision.header.schema_revision_id(),
            "schema_split_from",
        )
        .await;
        assert_eq!(origins, vec![source.revision.header.schema_revision_id().0]);
    }
    let source_after_split = read_schema(owner, subject, source.header.schema_id()).await;
    assert_eq!(
        source_after_split.header.lifecycle.acceptance,
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
    let ids: Vec<_> = split.iter().map(|child| child.header.schema_id()).collect();
    let epochs: Vec<_> = split
        .iter()
        .map(|child| child.header.object_epoch)
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
        let unchanged = read_schema(owner, subject, child.header.schema_id()).await;
        assert_eq!(unchanged.header.object_epoch, child.header.object_epoch);
        assert_eq!(
            unchanged.header.lifecycle.acceptance,
            AcceptanceState::Accepted
        );
    }
    let operation = OperationId::new();
    let merged_result = owner
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
    assert_eq!(replay, merged_result);
    let merged = match read_mutation_cognition(owner, subject, &merged_result).await {
        CognitionView::CognitiveSchema(value) => value,
        _ => panic!("schema mutation result should materialize as CognitiveSchema"),
    };
    assert!(merged.revision.header.producer_signature_id.is_some());
    assert_eq!(merged.evidence_links.len(), source.evidence_links.len());
    let origins = lineage(
        rt,
        merged.revision.header.schema_revision_id(),
        "schema_merged_from",
    )
    .await;
    assert_eq!(origins.len(), 2);
    for child in split {
        assert!(origins.contains(&child.revision.header.schema_revision_id().0));
        let withdrawn = read_schema(owner, subject, child.header.schema_id()).await;
        assert_eq!(
            withdrawn.header.lifecycle.acceptance,
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
