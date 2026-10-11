// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_kernel::NousRuntime;
use nous_material::ObservationMaterial;
use nous_memory::{EpisodePartitionSource, EpisodeView};
use nous_runtime::ManualCognitiveClock;
use std::sync::Arc;
use test_support::database;

use test_support::longitudinal::{
    consolidation_memory, consolidation_producer, consolidation_schema, journal_source_episode,
    observation, runtime_with_clock_serving,
};
use test_support::query::subject as create_subject;

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
    let schema = owner.create_schema(schema_input.clone()).await.unwrap();
    let mut journal_input = nous_memory::JournalInput {
        operation_id: OperationId::new(),
        subject,
        expected_authority_seq: rt.store.authority_seq(subject).await.unwrap(),
        target: None,
        sources: vec![EpisodePartitionSource {
            revision: episode.revision.episode_revision_id,
            expected_epoch: episode.object.object_epoch,
        }],
        title: None,
        narrative: "archival past journal".into(),
        points: vec![nous_memory::JournalPoint {
            role: nous_memory::JournalPointRole::Summary,
            text: "archival past summary".into(),
            basis: vec![RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: CognitiveRef::EpisodeRevision(
                    episode.revision.episode_revision_id,
                ),
                basis_role: BasisRole::Direct,
            })],
        }],
        producer: None,
    };
    let journal = owner.commit_journal(journal_input.clone()).await.unwrap();
    let cut = rt.cognition.now(subject);
    let old = vec![
        CognitiveRef::MemoryRevision(memory.revision.memory_revision_id),
        CognitiveRef::CognitiveSchemaRevision(schema.revision.schema_revision_id),
        CognitiveRef::EpisodeRevision(episode.revision.episode_revision_id),
        CognitiveRef::JournalRevision(journal.revision.journal_revision_id),
    ];
    clock.advance_by(subject, Duration::seconds(10)).unwrap();
    let new_schema = owner
        .revise_schema(nous_memory::ReviseSchemaInput {
            operation_id: OperationId::new(),
            subject,
            schema_id: schema.schema.schema_id,
            expected_object_epoch: schema.schema.object_epoch,
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
    let current_journal = owner
        .journal(subject, journal.object.journal_id, None)
        .await
        .unwrap();
    journal_input.operation_id = OperationId::new();
    journal_input.expected_authority_seq = rt.store.authority_seq(subject).await.unwrap();
    journal_input.target = Some(nous_memory::JournalTarget {
        journal_id: journal.object.journal_id,
        expected_revision: journal.revision.journal_revision_id,
        expected_epoch: current_journal.object.object_epoch,
        intent: "revalidate".into(),
    });
    journal_input.sources = vec![EpisodePartitionSource {
        revision: new_episode.revision.episode_revision_id,
        expected_epoch: new_episode.object.object_epoch,
    }];
    journal_input.narrative = "archival future journal".into();
    journal_input.points[0].basis = vec![RevisionBasis::CognitionDependency(CognitionDependency {
        epistemic_relation: None,
        target_revision: CognitiveRef::EpisodeRevision(new_episode.revision.episode_revision_id),
        basis_role: BasisRole::Direct,
    })];
    let new_journal = owner.commit_journal(journal_input).await.unwrap();
    let new = vec![
        CognitiveRef::CognitiveSchemaRevision(new_schema.revision.schema_revision_id),
        CognitiveRef::EpisodeRevision(new_episode.revision.episode_revision_id),
        CognitiveRef::JournalRevision(new_journal.revision.journal_revision_id),
    ];
    assert_historical_domains(&rt, subject, cut, &old, &new).await;
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
    rt.require_memory()
        .unwrap()
        .revise_episode(nous_memory::ReviseEpisodeInput {
            operation_id: nous_core::OperationId::new(),
            subject,
            episode_id: episode.object.episode_id,
            expected_object_epoch: episode.object.object_epoch,
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
        .unwrap()
}

#[tokio::test]
async fn episode_media_synopsis_tracks_ready_derivation_without_revising_authority() {
    use nous_core::CognitiveRef;
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock_serving(&url, &root, clock.clone(), true).await;
    let subject = create_subject(&rt).await;
    let session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    let region = episode_media_source(&rt, subject, session.session_id).await;
    let episode = rt
        .organize_experience(subject, 128, true)
        .await
        .unwrap()
        .episodes
        .remove(0);
    sqlx::query("INSERT INTO coverage_needs(coverage_need_id,subject_id,source_region_id,representation_kind,capability_operation,requirement,state,updated_at) VALUES($1,$2,$3,'image_description','image_interpretation','preferred','missing',$4)")
        .bind(uuid::Uuid::new_v4()).bind(subject.0).bind(region.0).bind(rt.cognition.now(subject)).execute(rt.store.pool()).await.unwrap();
    let query = media_episode_query(subject, "cobalt");
    assert!(rt.query(query.clone()).await.unwrap().results.is_empty());
    let synopsis = format!(
        "cobalt harbor {}界",
        "x".repeat(2047 - "cobalt harbor ".len())
    );
    let first = persist_media_synopsis(&rt, subject, region, &synopsis, None, "1").await;
    let results = rt.query(query.clone()).await.unwrap().results;
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].reference,
        CognitiveRef::EpisodeRevision(episode.revision.episode_revision_id)
    );
    let text = results[0].representation.as_ref().unwrap();
    assert!(text.contains("cobalt harbor"));
    assert!(!text.contains('界'));
    assert!(results[0].evidence.iter().any(|basis| basis.reference
        == CognitiveRef::DerivedRepresentation(first)
        && basis.basis_role == "interpretation"));
    let fragments = rt
        .store
        .episode_member_text_input(
            subject,
            &[episode.revision.episode_revision_id.0],
            rt.configuration
                .snapshot_for_subject(subject)
                .unwrap()
                .get(nous_memory::EPISODE_SYNOPSIS)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        fragments[&episode.revision.episode_revision_id.0][0].reference,
        CognitiveRef::DerivedRepresentation(first)
    );
    let selected = rt
        .material
        .text_excerpt(
            subject,
            &CognitiveRef::DerivedRepresentation(first),
            2048,
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.text.len(), 2047);
    assert_synopsis_policy(&rt, subject).await;
    let historical_cut = rt.cognition.now(subject);
    clock.advance_by(subject, Duration::seconds(10)).unwrap();
    let second =
        persist_media_synopsis(&rt, subject, region, "amber inlet", Some(first), "2").await;
    assert_historical_synopsis(&rt, subject, historical_cut, first, second).await;
    assert!(rt.query(query).await.unwrap().results.is_empty());
    let result = rt
        .query(media_episode_query(subject, "amber"))
        .await
        .unwrap();
    assert_eq!(
        result.results[0].reference,
        CognitiveRef::EpisodeRevision(episode.revision.episode_revision_id)
    );
    assert!(
        result.results[0]
            .representation
            .as_ref()
            .unwrap()
            .contains("amber inlet")
    );
    let current = rt
        .require_memory()
        .unwrap()
        .episode(subject, episode.object.episode_id, None)
        .await
        .unwrap();
    assert_eq!(current.object.object_epoch, episode.object.object_epoch);
    assert_eq!(
        current.revision.episode_revision_id,
        episode.revision.episode_revision_id
    );
    assert_eq!(
        rt.require_memory()
            .unwrap()
            .provenance_summary(subject, &current.basis)
            .await
            .unwrap()
            .roots
            .len(),
        1
    );
}

async fn assert_synopsis_policy(rt: &NousRuntime, subject: nous_core::SubjectId) {
    let mut budget = rt
        .configuration
        .snapshot_for_subject(subject)
        .unwrap()
        .get(nous_memory::EPISODE_SYNOPSIS)
        .unwrap();
    budget.fragment_max_bytes = 7;
    rt.configuration
        .set_subject_override(
            nous_core::OperationId::new(),
            subject,
            nous_memory::EPISODE_SYNOPSIS.path(),
            serde_json::to_value(budget).unwrap(),
            None,
        )
        .await
        .unwrap();
    assert!(
        rt.query(media_episode_query(subject, "harbor"))
            .await
            .unwrap()
            .results
            .is_empty()
    );
    let results = rt
        .query(media_episode_query(subject, "cobalt"))
        .await
        .unwrap()
        .results;
    assert_eq!(results.len(), 1);
    assert!(
        !results[0]
            .representation
            .as_ref()
            .unwrap()
            .contains("harbor")
    );
    rt.configuration
        .clear_subject_override(
            nous_core::OperationId::new(),
            subject,
            nous_memory::EPISODE_SYNOPSIS.path(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        rt.query(media_episode_query(subject, "harbor"))
            .await
            .unwrap()
            .results
            .len(),
        1
    );
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

async fn persist_media_synopsis(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    region: nous_core::SourceRegionId,
    text: &str,
    supersedes: Option<nous_core::DerivedRepresentationId>,
    model_revision: &str,
) -> nous_core::DerivedRepresentationId {
    let mut producer = consolidation_producer();
    producer.operation = nous_core::CapabilityOperation::ImageInterpretation;
    producer.implementation = "longitudinal-media-test".into();
    producer.model_revision = Some(model_revision.into());
    rt.material
        .persist_derived_representation(nous_material::DerivedRepresentation {
            derived_representation_id: nous_core::DerivedRepresentationId::new(),
            subject_id: subject,
            inputs: vec![nous_material::DerivationInput {
                ordinal: 0,
                reference: nous_core::CognitiveRef::SourceRegion(region),
                role: "source".into(),
            }],
            strategy: "direct_multimodal".into(),
            representation_kind: nous_core::RepresentationKind::ImageDescription,
            producer,
            revision: 1,
            payload_text: Some(text.into()),
            payload_json: None,
            payload_artifact_id: None,
            quality: serde_json::json!({}),
            created_at: Utc::now(),
            supersedes,
        })
        .await
        .unwrap()
        .derived_representation_id
}

async fn assert_historical_synopsis(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    cut: chrono::DateTime<Utc>,
    old: nous_core::DerivedRepresentationId,
    new: nous_core::DerivedRepresentationId,
) {
    use nous_core::*;
    let view = rt
        .historical_authority_view(subject, cut, RevisionView::Current)
        .await
        .unwrap();
    assert!(
        view.material_documents
            .contains(&CognitiveRef::DerivedRepresentation(old))
    );
    assert!(
        !view
            .material_visibility
            .contains(&CognitiveRef::DerivedRepresentation(new))
    );
    let current = rt
        .material
        .project_as_of(subject, rt.cognition.now(subject))
        .await
        .unwrap();
    assert!(
        !current
            .document_references
            .contains(&CognitiveRef::DerivedRepresentation(old))
    );
    assert!(
        current
            .document_references
            .contains(&CognitiveRef::DerivedRepresentation(new))
    );
    let mut query = media_episode_query(subject, "cobalt");
    query.temporal_frame.authority_view = AuthorityView::AsOf(cut);
    let result = rt.query(query).await.unwrap();
    assert!(result.results.iter().any(|hit| {
        hit.representation
            .as_ref()
            .is_some_and(|text| text.contains("cobalt harbor"))
    }));
    assert!(!result.results.iter().any(|hit| {
        hit.representation
            .as_ref()
            .is_some_and(|text| text.contains("amber inlet"))
    }));
}

async fn episode_media_source(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    session: nous_core::SessionId,
) -> nous_core::SourceRegionId {
    let artifact = rt
        .material
        .upload_stream(
            subject,
            nous_material::UploadMetadata {
                media_type: "image/png".into(),
                metadata: serde_json::json!({}),
            },
            futures::stream::iter([Ok(vec![137, 80, 78, 71, 13, 10, 26, 10])]),
        )
        .await
        .unwrap();
    let mut input = observation(subject, Some(session));
    input.material = ObservationMaterial::ArtifactRef {
        artifact_id: artifact.artifact_id,
    };
    let observed = rt.material.record_observation(input).await.unwrap();
    observed.source_region.unwrap().source_region_id
}
