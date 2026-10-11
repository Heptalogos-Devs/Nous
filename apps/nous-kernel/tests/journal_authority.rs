// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_core::TemporalExtent;
use nous_kernel::NousRuntime;
use nous_memory::{EpisodePartitionSource, EpisodeView};
use nous_runtime::ManualCognitiveClock;
use sqlx::Row;
use std::sync::Arc;
use test_support::database;

use test_support::longitudinal::{
    consolidation_memory, consolidation_schema, journal_source_episode, runtime_with_clock_serving,
};
use test_support::query::subject as create_subject;

#[tokio::test]
async fn journal_lineage_revalidation_and_receipt_are_exact() {
    use nous_core::{
        BasisRole, CognitionDependency, CognitiveRef, IntegrityState, OperationId, RevisionBasis,
    };
    use nous_memory::{JournalInput, JournalPoint, JournalPointRole};
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock_serving(&url, &root, clock.clone(), true).await;
    let subject = create_subject(&rt).await;
    let episode = journal_source_episode(&rt, &clock, subject).await;
    let memory = rt.require_memory().unwrap();
    let basis = RevisionBasis::CognitionDependency(CognitionDependency {
        epistemic_relation: None,
        target_revision: CognitiveRef::EpisodeRevision(episode.revision.episode_revision_id),
        basis_role: BasisRole::Direct,
    });
    let input = JournalInput {
        operation_id: OperationId::new(),
        subject,
        expected_authority_seq: rt.store.authority_seq(subject).await.unwrap(),
        target: None,
        sources: vec![EpisodePartitionSource {
            revision: episode.revision.episode_revision_id,
            expected_epoch: episode.object.object_epoch,
        }],
        title: Some("Experience summary".into()),
        narrative: "Two sources contributed.".into(),
        points: vec![JournalPoint {
            role: JournalPointRole::Summary,
            text: "Point-only detail: two independent sources.".into(),
            basis: vec![basis],
        }],
        producer: None,
    };
    let journal = memory.commit_journal(input.clone()).await.unwrap();
    let mut recall = test_support::query::query(subject);
    recall.expression.targets = vec![nous_core::QueryTarget::Exact {
        reference: CognitiveRef::JournalRevision(journal.revision.journal_revision_id),
    }];
    assert_eq!(rt.query(recall.clone()).await.unwrap().results.len(), 1);
    let dependencies = create_journal_dependents(&rt, subject, &episode, &journal).await;
    let basis = RevisionBasis::CognitionDependency(CognitionDependency {
        epistemic_relation: None,
        target_revision: CognitiveRef::JournalRevision(journal.revision.journal_revision_id),
        basis_role: BasisRole::Direct,
    });
    let provenance = memory
        .provenance_summary(subject, std::slice::from_ref(&basis))
        .await
        .unwrap();
    assert_eq!(provenance.roots.len(), 2);
    assert_eq!(provenance.normalized_inputs.len(), 1);
    let mut invalid = input.clone();
    invalid.operation_id = OperationId::new();
    invalid.expected_authority_seq = rt.store.authority_seq(subject).await.unwrap();
    invalid.points[0].basis = vec![basis];
    assert!(memory.commit_journal(invalid).await.is_err());
    memory
        .suppress_episode(
            subject,
            episode.object.episode_id,
            OperationId::new(),
            episode.object.object_epoch,
        )
        .await
        .unwrap();
    let invalidated = memory
        .journal(subject, journal.object.journal_id, None)
        .await
        .unwrap();
    assert!(matches!(
        invalidated.object.integrity_state,
        IntegrityState::RevalidationRequired
    ));
    assert_eq!(
        invalidated.object.object_epoch,
        journal.object.object_epoch + 1
    );
    assert_eq!(invalidated.revision.narrative, journal.revision.narrative);
    assert!(rt.query(recall).await.unwrap().results.is_empty());
    assert_invalidated_dependents(&rt, subject, &dependencies).await;
    let needs = rt.cognition.maintenance_needs(subject).await.unwrap();
    assert!(needs.iter().any(|need| need.kind == "journal_revalidate"
        && need.scope_ref == journal.object.journal_id.0.to_string()));
    clock.advance_by(subject, Duration::days(1)).unwrap();
    let replay = memory.commit_journal(input.clone()).await.unwrap();
    assert_eq!(
        replay.revision.journal_revision_id,
        journal.revision.journal_revision_id
    );
    assert_eq!(replay.revision.formed_at, journal.revision.formed_at);
    assert_eq!(replay.revision.recorded_at, journal.revision.recorded_at);
    let revised = assert_journal_revalidation(&rt, input, &episode, &invalidated).await;
    let uses = assert_longitudinal_use(&rt, subject, &episode, &revised).await;
    assert_journal_purge(&rt, subject, &revised, &episode, uses, &dependencies).await;
}

async fn assert_journal_revalidation(
    rt: &NousRuntime,
    mut input: nous_memory::JournalInput,
    episode: &EpisodeView,
    invalidated: &nous_memory::JournalView,
) -> nous_memory::JournalView {
    let subject = input.subject;
    let memory = rt.require_memory().unwrap();
    let restored = memory
        .restore_episode(
            subject,
            episode.object.episode_id,
            nous_core::OperationId::new(),
            episode.object.object_epoch + 1,
        )
        .await
        .unwrap();
    input.operation_id = nous_core::OperationId::new();
    input.expected_authority_seq = rt.store.authority_seq(subject).await.unwrap();
    input.sources[0].expected_epoch = restored.object.object_epoch;
    input.target = Some(nous_memory::JournalTarget {
        journal_id: invalidated.object.journal_id,
        expected_revision: invalidated.revision.journal_revision_id,
        expected_epoch: invalidated.object.object_epoch,
        intent: "revalidate".into(),
    });
    let revised = memory.commit_journal(input.clone()).await.unwrap();
    assert_eq!(revised.object.journal_id, invalidated.object.journal_id);
    assert_ne!(
        revised.revision.journal_revision_id,
        invalidated.revision.journal_revision_id
    );
    assert!(matches!(
        revised.object.integrity_state,
        nous_core::IntegrityState::Valid
    ));
    assert_eq!(
        memory
            .journal_history(subject, revised.object.journal_id)
            .await
            .unwrap()
            .len(),
        2
    );
    input.operation_id = nous_core::OperationId::new();
    input.expected_authority_seq = rt.store.authority_seq(subject).await.unwrap();
    assert!(memory.commit_journal(input).await.is_err());
    revised
}

async fn assert_longitudinal_use(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    episode: &EpisodeView,
    journal: &nous_memory::JournalView,
) -> nous_runtime::UseFeedback {
    let references = vec![
        nous_core::CognitiveRef::EpisodeRevision(episode.revision.episode_revision_id),
        nous_core::CognitiveRef::JournalRevision(journal.revision.journal_revision_id),
    ];
    let context = rt
        .cognition
        .create_work_context(nous_runtime::CreateWorkContextInput {
            operation_id: nous_core::OperationId::new(),
            subject,
            purpose: "Continue longitudinal cognition".into(),
            unresolved_questions: vec![],
            constraints: serde_json::json!({}),
            resume_conditions: vec![],
            budget_summary: serde_json::json!({}),
            context_text: String::new(),
            entity_anchors: vec![],
            tag_anchors: vec![],
            cognition_anchors: references.clone(),
        })
        .await
        .unwrap();
    assert_eq!(context.cognition_anchors, references);
    let session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    let input = nous_runtime::UseFeedback {
        subject,
        session_id: Some(session.session_id),
        consumer_ref: "consumer:test:longitudinal".into(),
        events: references
            .into_iter()
            .map(|reference| nous_runtime::UseFeedbackEvent {
                query_id: None,
                event_id: nous_core::UseEventId::new(),
                reference,
                use_kind: nous_runtime::UseKind::Referenced,
                occurred_at: rt.cognition.now(subject),
                context: serde_json::json!({}),
            })
            .collect(),
    };
    let result = rt.cognition.use_feedback(input.clone()).await.unwrap();
    assert_eq!((result.0, result.1), (2, 0));
    let result = rt.cognition.use_feedback(input.clone()).await.unwrap();
    assert_eq!((result.0, result.1), (0, 2));
    assert_eq!(
        rt.cognition
            .session(subject, session.session_id)
            .await
            .unwrap()
            .resident
            .len(),
        2
    );
    input
}

async fn create_journal_dependents(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    episode: &EpisodeView,
    journal: &nous_memory::JournalView,
) -> Vec<nous_core::CognitiveRef> {
    use nous_core::{BasisRole, CognitionDependency, CognitiveRef, OperationId, RevisionBasis};
    use nous_memory::FormationMode;
    let journal_ref = CognitiveRef::JournalRevision(journal.revision.journal_revision_id);
    let mut content = consolidation_memory(episode);
    content.formation_mode = FormationMode::Synthesized;
    content.grounding_occurrence_id = None;
    content.basis = [
        journal_ref.clone(),
        CognitiveRef::EpisodeRevision(episode.revision.episode_revision_id),
    ]
    .into_iter()
    .map(|target_revision| {
        RevisionBasis::CognitionDependency(CognitionDependency {
            epistemic_relation: None,
            target_revision,
            basis_role: BasisRole::Direct,
        })
    })
    .collect();
    let memory = rt.require_memory().unwrap();
    let mut refs = Vec::new();
    for _ in 0..2 {
        let mut input = content.clone();
        input.operation_id = OperationId::new();
        let result = memory.form_memory(input).await.unwrap();
        refs.push(CognitiveRef::MemoryRevision(
            result.revision.memory_revision_id,
        ));
    }
    let mut schema = consolidation_schema(episode);
    schema.content.evidence_links = refs
        .iter()
        .cloned()
        .map(|target_revision| nous_memory::SchemaEvidenceLinkInput {
            role: nous_memory::SchemaEvidenceRole::Support,
            basis: RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision,
                basis_role: BasisRole::Direct,
            }),
        })
        .collect();
    let result = memory.create_schema(schema).await.unwrap();
    refs.push(CognitiveRef::CognitiveSchemaRevision(
        result.revision.schema_revision_id,
    ));
    let episode = memory
        .create_episode(nous_memory::EpisodeInput {
            operation_id: OperationId::new(),
            subject,
            track_key: "manual-dependent".into(),
            title: None,
            parent_episode_revision_id: None,
            experience_time: TemporalExtent::Unknown,
            boundary_explanation: "Supported cognitive continuation.".into(),
            producer_signature_id: None,
            members: vec![nous_memory::EpisodeMemberInput {
                reference: refs[0].clone(),
                role: "context".into(),
            }],
            basis: vec![RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: refs[0].clone(),
                basis_role: BasisRole::Direct,
            })],
        })
        .await
        .unwrap();
    refs.push(CognitiveRef::EpisodeRevision(
        episode.revision.episode_revision_id,
    ));
    refs
}

async fn assert_invalidated_dependents(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    refs: &[nous_core::CognitiveRef],
) {
    for reference in refs {
        let query = match reference {
            nous_core::CognitiveRef::MemoryRevision(_) => {
                "SELECT o.integrity_state,o.object_epoch FROM memory_objects o JOIN memory_revisions r USING(memory_id) WHERE o.subject_id=$1 AND r.memory_revision_id=$2"
            }
            nous_core::CognitiveRef::CognitiveSchemaRevision(_) => {
                "SELECT o.integrity_state,o.object_epoch FROM cognitive_schemas o JOIN cognitive_schema_revisions r USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2"
            }
            nous_core::CognitiveRef::EpisodeRevision(_) => {
                "SELECT o.integrity_state,o.object_epoch FROM episode_objects o JOIN episode_revisions r USING(episode_id) WHERE o.subject_id=$1 AND r.episode_revision_id=$2"
            }
            _ => panic!("unexpected dependency kind"),
        };
        let id = reference
            .to_string()
            .split_once(':')
            .unwrap()
            .1
            .parse::<uuid::Uuid>()
            .unwrap();
        let row = sqlx::query(query)
            .bind(subject.0)
            .bind(id)
            .fetch_one(rt.store.pool())
            .await
            .unwrap();
        assert_eq!(
            row.get::<String, _>("integrity_state"),
            "revalidation_required"
        );
        assert_eq!(row.get::<i64, _>("object_epoch"), 2);
    }
    let schemas = refs
        .iter()
        .filter_map(|reference| match reference {
            nous_core::CognitiveRef::CognitiveSchemaRevision(id) => Some(id.0.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let edges: i64 = sqlx::query_scalar("SELECT count(*) FROM cognition_dependency_invalidations WHERE subject_id=$1 AND dependent_kind='cognitive_schema_revision' AND dependent_ref=ANY($2::text[]) AND invalidated_by_kind='memory_revision'")
        .bind(subject.0).bind(schemas).fetch_one(rt.store.pool()).await.unwrap();
    assert_eq!(edges, 2);
}

async fn assert_journal_purge(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    journal: &nous_memory::JournalView,
    episode: &EpisodeView,
    uses: nous_runtime::UseFeedback,
    dependencies: &[nous_core::CognitiveRef],
) {
    use nous_core::OperationId;
    let memory = rt.require_memory().unwrap();
    let fresh_dependencies = create_journal_dependents(rt, subject, episode, journal).await;
    let before = rt.store.authority_seq(subject).await.unwrap();
    memory
        .purge_journal(
            subject,
            journal.object.journal_id,
            OperationId::new(),
            journal.object.object_epoch,
        )
        .await
        .unwrap();
    assert!(
        memory
            .journal(subject, journal.object.journal_id, None)
            .await
            .is_err()
    );
    assert!(
        memory
            .episode_revision(subject, episode.revision.episode_revision_id)
            .await
            .is_ok()
    );
    assert_eq!(rt.cognition.use_feedback(uses).await.unwrap().1, 2);
    assert_invalidated_dependents(rt, subject, dependencies).await;
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), before + 1);
    assert_invalidated_dependents(rt, subject, &fresh_dependencies).await;
}
