// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_kernel::NousRuntime;
use nous_memory::{CognitionView, EpisodePartitionSource, EpisodeView};
use nous_runtime::ManualCognitiveClock;
use std::sync::Arc;
use test_support::database;

use test_support::longitudinal::{
    consolidation_memory, consolidation_producer, consolidation_schema, journal_source_episode,
    runtime_with_clock_serving,
};
use test_support::query::subject as create_subject;
use test_support::read_mutation_cognition;

#[tokio::test]
async fn historical_schema_episode_journal_use_past_heads_and_history_documents() {
    use nous_core::{BasisRole, CognitionDependency, CognitiveRef, OperationId, RevisionBasis};
    let (root, url, _pg) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock_serving(&url, &root, clock.clone(), true).await;
    let subject = create_subject(&rt).await;
    let episode = journal_source_episode(&rt, &clock, subject).await;
    let episode = historical_episode_text(&rt, subject, &episode, "archival past episode").await;
    let owner = rt.require_memory().unwrap();
    let mut memory_input = consolidation_memory(&episode);
    memory_input.representation_text = "archival past memory".into();
    let memory = owner.form_memory(memory_input).await.unwrap();
    let mut schema_input = consolidation_schema(&episode);
    schema_input.content.structural_claim = "archival past schema".into();
    let schema_result = owner.create_schema(schema_input.clone()).await.unwrap();
    let schema = match read_mutation_cognition(owner, subject, &schema_result).await {
        CognitionView::CognitiveSchema(value) => value,
        _ => panic!("schema mutation result should materialize as CognitiveSchema"),
    };
    let mut journal_input = nous_memory::JournalInput {
        operation_id: OperationId::new(),
        subject,
        expected_authority_seq: rt.store.authority_seq(subject).await.unwrap(),
        target: None,
        sources: vec![EpisodePartitionSource {
            revision: episode.revision.header.episode_revision_id(),
            expected_epoch: episode.header.object_epoch,
        }],
        title: None,
        narrative: "archival past journal".into(),
        points: vec![nous_memory::JournalPoint {
            role: nous_memory::JournalPointRole::Summary,
            text: "archival past summary".into(),
            basis: vec![RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: CognitiveRef::EpisodeRevision(
                    episode.revision.header.episode_revision_id(),
                ),
                basis_role: BasisRole::Direct,
            })],
        }],
        producer: None,
    };
    let journal = owner.commit_journal(journal_input.clone()).await.unwrap();
    let CognitiveRef::Journal(journal_id) = journal.reference else {
        panic!("journal mutation result should reference Journal");
    };
    let cut = rt.cognition.now(subject);
    let old = vec![
        CognitiveRef::MemoryRevision(nous_core::MemoryRevisionId(memory.revision_id.unwrap())),
        CognitiveRef::CognitiveSchemaRevision(nous_core::CognitiveSchemaRevisionId(
            schema_result.revision_id.unwrap(),
        )),
        CognitiveRef::EpisodeRevision(episode.revision.header.episode_revision_id()),
        CognitiveRef::JournalRevision(nous_core::JournalRevisionId(journal.revision_id.unwrap())),
    ];
    clock.advance_by(subject, Duration::seconds(10)).unwrap();
    let new_schema = owner
        .revise_schema(nous_memory::ReviseSchemaInput {
            operation_id: OperationId::new(),
            subject,
            schema_id: schema.header.schema_id(),
            expected_object_epoch: schema.header.object_epoch,
            intent: nous_memory::RevisionIntent::Rephrase,
            copy_link_ids: vec![],
            content: nous_memory::SchemaContent {
                title: None,
                structural_claim: "archival future schema".into(),
                applicability_scope: schema.revision.applicability_scope.clone(),
                boundary_definition: schema.revision.boundary_definition.clone(),
                formation_kind: schema.revision.formation_kind,
                producer: Some(consolidation_producer()),
                evidence_links: schema_input.content.evidence_links,
            },
        })
        .await
        .unwrap();
    let new_episode =
        historical_episode_text(&rt, subject, &episode, "archival future episode").await;
    let nous_memory::CognitionView::Journal(current_journal) = owner
        .read_cognition(subject, nous_core::CognitiveRef::Journal(journal_id))
        .await
        .unwrap()
    else {
        panic!("common Cognition read returned the wrong view kind");
    };
    journal_input.operation_id = OperationId::new();
    journal_input.expected_authority_seq = rt.store.authority_seq(subject).await.unwrap();
    journal_input.target = Some(nous_memory::JournalTarget {
        journal_id,
        expected_revision: nous_core::JournalRevisionId(journal.revision_id.unwrap()),
        expected_epoch: current_journal.header.object_epoch,
        intent: "revalidate".into(),
    });
    journal_input.sources = vec![EpisodePartitionSource {
        revision: new_episode.revision.header.episode_revision_id(),
        expected_epoch: new_episode.header.object_epoch,
    }];
    journal_input.narrative = "archival future journal".into();
    journal_input.points[0].basis = vec![RevisionBasis::CognitionDependency(CognitionDependency {
        epistemic_relation: None,
        target_revision: CognitiveRef::EpisodeRevision(
            new_episode.revision.header.episode_revision_id(),
        ),
        basis_role: BasisRole::Direct,
    })];
    let new_journal = owner.commit_journal(journal_input).await.unwrap();
    let new = vec![
        CognitiveRef::CognitiveSchemaRevision(nous_core::CognitiveSchemaRevisionId(
            new_schema.revision_id.unwrap(),
        )),
        CognitiveRef::EpisodeRevision(new_episode.revision.header.episode_revision_id()),
        CognitiveRef::JournalRevision(nous_core::JournalRevisionId(
            new_journal.revision_id.unwrap(),
        )),
    ];
    assert_historical_domains(&rt, subject, cut, &old, &new).await;
}

fn media_episode_query(subject: nous_core::SubjectId, text: &str) -> nous_core::CognitiveQuery {
    use nous_core::{CognitiveQuery, CognitiveQueryExpr, Cue, TextCue};
    let mut query = CognitiveQuery {
        projection: Default::default(),
        temporal_frame: Default::default(),
        work_context: None,
        subject,
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues: vec![Cue::Text(TextCue { text: text.into() })],
            ..Default::default()
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: Default::default(),
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: Default::default(),
    };
    query.projection.domains = vec![nous_core::ResultDomain::Episode];
    query.capabilities.text_embedding = nous_core::RequirementStrength::Forbidden;
    query
}

async fn assert_historical_domains(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    cut: chrono::DateTime<Utc>,
    old: &[nous_core::CognitiveRef],
    new: &[nous_core::CognitiveRef],
) {
    use nous_core::*;
    let mut query = media_episode_query(subject, "archival");
    query.projection = ResultProjection::default();
    query.result_need.limit = 16;
    query.temporal_frame.authority_view = AuthorityView::AsOf(cut);
    let view = rt
        .historical_authority_view(subject, cut, RevisionView::Current)
        .await
        .unwrap();
    for reference in old {
        let state = view.cognition_for(reference).unwrap();
        let mut exact = query.clone();
        exact.expression.cues = vec![Cue::Text(nous_core::TextCue {
            text: "read archival cognition".into(),
        })];
        exact.expression.targets = vec![QueryTarget::Exact {
            reference: state.object.clone(),
        }];
        let result = rt.execute_query(exact, None).await.unwrap();
        assert!(
            result
                .result
                .results
                .iter()
                .any(|hit| &hit.reference == reference),
            "missing historical owner hit {reference}: {:?}",
            result.result
        );
        assert!(
            result
                .result
                .results
                .iter()
                .any(|hit| hit.authority_epoch == state.state["object_epoch"].as_i64())
        );
    }
    query.temporal_frame.revision_view = RevisionView::History;
    let past = rt.execute_query(query.clone(), None).await.unwrap();
    assert!(past.result.generation.lexical.is_some());
    assert!(
        past.result
            .results
            .iter()
            .all(|hit| !new.contains(&hit.reference))
    );
    query.temporal_frame.authority_view = AuthorityView::Current;
    let history = rt.execute_query(query.clone(), None).await.unwrap();
    for reference in old.iter().chain(new) {
        assert!(
            history
                .result
                .results
                .iter()
                .any(|hit| &hit.reference == reference),
            "history missed {reference}: {:?}",
            history.result
        );
    }
    query.temporal_frame.revision_view = RevisionView::Current;
    let current = rt.execute_query(query.clone(), None).await.unwrap();
    for reference in new {
        assert!(
            current
                .result
                .results
                .iter()
                .any(|hit| &hit.reference == reference)
        );
    }
    for reference in &old[1..] {
        assert!(
            !current
                .result
                .results
                .iter()
                .any(|hit| &hit.reference == reference)
        );
    }
    query.temporal_frame.revision_view = RevisionView::History;
    query.expression.constraints.recorded = Some(TimePredicate::Range {
        start: None,
        end: Some(cut + Duration::seconds(1)),
    });
    let filtered = rt.execute_query(query, None).await.unwrap();
    assert!(
        filtered
            .result
            .results
            .iter()
            .all(|hit| !new.contains(&hit.reference))
    );
    assert!(!filtered.result.results.is_empty());
}

async fn historical_episode_text(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    episode: &EpisodeView,
    text: &str,
) -> EpisodeView {
    let result = rt
        .require_memory()
        .unwrap()
        .revise_episode(nous_memory::ReviseEpisodeInput {
            operation_id: nous_core::OperationId::new(),
            subject,
            episode_id: episode.header.episode_id(),
            expected_object_epoch: episode.header.object_epoch,
            intent: "reinterpret".into(),
            title: None,
            parent_episode_revision_id: None,
            experience_time: episode.revision.experience_time.clone(),
            boundary_explanation: text.into(),
            producer_signature_id: None,
            members: episode
                .members
                .iter()
                .map(|member| nous_memory::EpisodeMemberInput {
                    reference: member.reference.clone(),
                    role: member.role.clone(),
                })
                .collect(),
            basis: episode.basis.clone(),
        })
        .await
        .unwrap();
    match read_mutation_cognition(rt.require_memory().unwrap(), subject, &result).await {
        CognitionView::Episode(value) => value,
        _ => panic!("episode mutation result should materialize as Episode"),
    }
}
