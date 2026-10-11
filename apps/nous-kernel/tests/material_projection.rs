// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use chrono::{Duration, SubsecRound, Utc};
use nous_core::*;
use nous_material::{
    ByteRange, DerivationInput, DerivedRegion, DerivedRepresentation, MaterializeRequest,
    ObservationMaterial,
};
use nous_memory::{
    AssociationBasis, AssociationBasisClass, AssociationPolarity, CreateAssociationRequest,
};
use nous_runtime::{CognitiveClock, ManualCognitiveClock};
use std::{collections::BTreeSet, sync::Arc};
use test_support::longitudinal::{consolidation_producer, observation, runtime_with_clock_serving};
use test_support::query::{query, subject};
use test_support::{database, form_input};

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real owner cohort checks selected text, source independence and bounded cache work across current/history"
)]
async fn selected_regions_and_multisource_basis_are_shared_across_read_index_and_embedding() {
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock_serving(&url, &root, clock.clone(), true).await;
    let subject = subject(&rt).await;
    let body = "amberα cobalt蓝";
    let mut sources = Vec::new();
    for (text, external) in [
        (body, "object:amber-source"),
        ("independent source", "object:independent-source"),
    ] {
        let mut input = observation(subject, None);
        input.material = ObservationMaterial::InlineText {
            text: text.into(),
            media_type: "text/plain".into(),
        };
        input.occurrence.external_object_ref = Some(ObjectRef::new(external).unwrap());
        sources.push(rt.material.record_observation(input).await.unwrap());
        clock.advance_by(subject, Duration::seconds(1)).unwrap();
    }
    let artifact = sources[0].artifact.as_ref().unwrap().artifact_id;
    let amber = insert_source_region(&rt, subject, artifact, 0, "amberα".len() as u64).await;
    let cobalt_start = body.find("cobalt").unwrap() as u64;
    let cobalt =
        insert_source_region(&rt, subject, artifact, cobalt_start, body.len() as u64).await;
    let representation = rt
        .material
        .persist_derived_representation(DerivedRepresentation {
            derived_representation_id: DerivedRepresentationId::new(),
            subject_id: subject,
            inputs: sources
                .iter()
                .enumerate()
                .map(|(ordinal, source)| DerivationInput {
                    ordinal: ordinal as u32,
                    reference: CognitiveRef::SourceRegion(
                        source.source_region.as_ref().unwrap().source_region_id,
                    ),
                    role: "source".into(),
                })
                .collect(),
            strategy: "deterministic-test".into(),
            representation_kind: RepresentationKind::ExtractedText,
            producer: consolidation_producer(),
            revision: 1,
            payload_text: Some(body.into()),
            payload_json: None,
            payload_artifact_id: None,
            quality: serde_json::json!({}),
            created_at: clock.now(subject),
            supersedes: None,
        })
        .await
        .unwrap();
    let derived_amber = derived_region(
        &rt,
        subject,
        representation.derived_representation_id,
        0,
        "amberα".len() as u64,
    )
    .await;
    let derived_cobalt = derived_region(
        &rt,
        subject,
        representation.derived_representation_id,
        cobalt_start,
        body.len() as u64,
    )
    .await;
    let occurrence = sources[0].occurrence.occurrence_id;
    let evidence = EvidenceRef {
        occurrence_id: occurrence,
        locator: EvidenceLocator::DerivedRepresentation(representation.derived_representation_id),
        basis_role: BasisRole::Direct,
        epistemic_relation: None,
    };
    let owner = rt.require_memory().unwrap();
    let mut input = form_input(
        subject,
        occurrence,
        OperationId::new(),
        "A joint material fact",
    );
    input.epistemic_class = EpistemicClass::Derived;
    input.basis = vec![RevisionBasis::Evidence(evidence.clone())];
    let memory = owner.form_memory(input).await.unwrap();
    let memory_ref = CognitiveRef::MemoryRevision(memory.revision.memory_revision_id);
    let target = owner
        .form_memory(form_input(
            subject,
            sources[1].occurrence.occurrence_id,
            OperationId::new(),
            "An independent target fact",
        ))
        .await
        .unwrap();
    let association = owner
        .create_association(
            CreateAssociationRequest {
                producer: None,
                operation_id: OperationId::new(),
                from: memory_ref.clone(),
                to: CognitiveRef::MemoryRevision(target.revision.memory_revision_id),
                relation_kind: "assoc.related".into(),
                polarity: AssociationPolarity::Positive,
                basis_class: AssociationBasisClass::SourceEvidence,
                basis: vec![AssociationBasis::Revision(RevisionBasis::Evidence(
                    evidence,
                ))],
                producer_signature_id: None,
                valid_time: TemporalExtent::Unknown,
            },
            subject,
        )
        .await
        .unwrap();
    let cut = clock.now(subject);
    let view = rt
        .historical_authority_view(subject, cut, RevisionView::Current)
        .await
        .unwrap();
    let region_refs = [
        CognitiveRef::SourceRegion(amber),
        CognitiveRef::SourceRegion(cobalt),
        CognitiveRef::DerivedRegion(derived_amber),
        CognitiveRef::DerivedRegion(derived_cobalt),
    ];
    let expected = ["amberα", "cobalt蓝", "amberα", "cobalt蓝"];
    for selected_view in [None, Some(&view)] {
        for (reference, text) in region_refs.iter().zip(expected) {
            let selected = rt
                .material
                .text_view(subject, reference, selected_view)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(selected.text, text);
            assert_eq!(
                selected.content_identity,
                nous_material::text_content_identity(text)
            );
            let exact = rt
                .material
                .materialize_in_view(
                    subject,
                    MaterializeRequest {
                        reference: reference.clone(),
                        byte_range: None,
                        max_bytes: 64,
                        resource_handle: None,
                        resource: None,
                    },
                    selected_view,
                )
                .await
                .unwrap();
            assert_eq!(exact.bytes, text.as_bytes());
            assert_eq!(
                (exact.byte_range.start, exact.byte_range.end),
                (selected.selection.start, selected.selection.end)
            );
        }
        let roots = owner
            .provenance_for_reference(subject, &memory_ref, selected_view)
            .await
            .unwrap();
        assert_eq!(roots.len(), 2);
        assert!(
            roots
                .iter()
                .all(|root| root.certainty == EvidenceRootCertainty::Known)
        );
        assert_eq!(
            owner
                .provenance_for_reference(
                    subject,
                    &CognitiveRef::Association(association.association_evidence_id),
                    selected_view
                )
                .await
                .unwrap(),
            roots
        );
        let projection = rt
            .serving
            .projection_input(
                subject,
                true,
                rt.configuration
                    .snapshot_for_subject(subject)
                    .unwrap()
                    .get(nous_configuration::ConfigKey::<
                        nous_persistence::EpisodeTextBudget,
                    >::new("episode.synopsis"))
                    .unwrap(),
                selected_view,
            )
            .await
            .unwrap();
        let expected_roots: BTreeSet<_> = roots.into_iter().map(|root| root.root_key).collect();
        assert_eq!(
            projection.evidence_roots[&memory_ref]
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            expected_roots
        );
        assert_eq!(
            projection
                .topology
                .edges
                .iter()
                .filter(|edge| edge.from == memory_ref && edge.association_kind == "assoc.related")
                .filter_map(|edge| edge.provenance_root.clone())
                .collect::<BTreeSet<_>>(),
            expected_roots
        );
        let mut input = query(subject);
        input.projection.domains = vec![ResultDomain::Evidence];
        input.expression.cues = vec![Cue::Text(TextCue {
            text: "cobalt蓝".into(),
        })];
        input.capabilities.text_embedding = RequirementStrength::Forbidden;
        input.temporal_frame.authority_view = selected_view
            .map_or(AuthorityView::Current, |view| {
                AuthorityView::AsOf(view.as_of)
            });
        let result = rt.query(input).await.unwrap();
        assert!(
            result
                .results
                .iter()
                .any(|hit| hit.reference == region_refs[1])
        );
        assert!(
            result
                .results
                .iter()
                .any(|hit| hit.reference == region_refs[3])
        );
        assert!(
            !result
                .results
                .iter()
                .any(|hit| hit.reference == region_refs[0] || hit.reference == region_refs[2])
        );
    }
    let capped = rt
        .material
        .text_excerpt(subject, &region_refs[0], 6, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(capped.text, "amber");
    assert_eq!(capped.selection.end, 5);
    assert!(matches!(
        rt.material
            .materialize(
                subject,
                MaterializeRequest {
                    reference: region_refs[0].clone(),
                    byte_range: Some(ByteRange { start: 6, end: 7 }),
                    max_bytes: 64,
                    resource_handle: None,
                    resource: None
                }
            )
            .await,
        Err(Error::Invalid(_))
    ));
    let mut cursor = None;
    let mut need = None;
    loop {
        let page = rt
            .serving
            .embedding_needs_page(subject, 2, cursor.as_ref(), None)
            .await
            .unwrap();
        assert!(page.needs.len() <= 2);
        for candidate in page.needs {
            if let Some(index) = region_refs
                .iter()
                .position(|reference| reference == &candidate.reference)
            {
                assert_eq!(candidate.text, expected[index]);
                need.get_or_insert(candidate);
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let need = need.expect("one selected region has missing model material");
    // A malformed later document must not be touched by a one-item page or another reference's commit.
    insert_source_region(&rt, subject, artifact, 0, body.len() as u64 + 1).await;
    let page = rt
        .serving
        .embedding_needs_page(subject, 1, None, None)
        .await
        .unwrap();
    assert!(page.needs.len() <= 1);
    let first = page.needs.first().unwrap();
    let provider = rt.serving.embedding().unwrap();
    rt.serving
        .commit_embedding(
            subject,
            first.reference.clone(),
            first.text.clone(),
            &provider.space().space_hash,
            &provider.producer().signature_hash,
            vec![1.0, 0.0],
        )
        .await
        .unwrap();
    let cached = rt
        .serving
        .embedding_needs_page(subject, 1, None, None)
        .await
        .unwrap();
    assert!(cached.needs.is_empty());
    assert!(cached.next_cursor.is_some());
    rt.serving
        .commit_embedding(
            subject,
            need.reference.clone(),
            need.text.clone(),
            &provider.space().space_hash,
            &provider.producer().signature_hash,
            vec![1.0, 0.0],
        )
        .await
        .unwrap();
    assert!(
        rt.serving
            .commit_embedding(
                subject,
                need.reference,
                format!("{} changed", need.text),
                &provider.space().space_hash,
                &provider.producer().signature_hash,
                vec![1.0, 0.0]
            )
            .await
            .is_err()
    );
    assert!(
        rt.serving
            .embedding_needs_page(subject, 1, page.next_cursor.as_ref(), Some(&view))
            .await
            .is_err()
    );
}

async fn insert_source_region(
    rt: &nous_kernel::NousRuntime,
    subject: SubjectId,
    artifact: ArtifactId,
    start: u64,
    end: u64,
) -> SourceRegionId {
    let region = SourceRegionId::new();
    let coordinate = serde_json::json!({"start":start,"end":end});
    let mut tx = rt.store.begin().await.unwrap();
    sqlx::query("INSERT INTO source_regions(source_region_id,subject_id,artifact_id,coordinate_kind,coordinate,coordinate_hash,created_at) VALUES($1,$2,$3,'byte_range',$4,$5,$6)")
        .bind(region.0).bind(subject.0).bind(artifact.0).bind(&coordinate).bind(coordinate.to_string()).bind(rt.cognition.now(subject)).execute(&mut *tx).await.unwrap();
    nous_persistence::AuthorityStore::invalidate_in(
        &mut tx,
        subject,
        nous_persistence::ProjectionInvalidation::text(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    region
}
async fn derived_region(
    rt: &nous_kernel::NousRuntime,
    subject: SubjectId,
    representation: DerivedRepresentationId,
    start: u64,
    end: u64,
) -> DerivedRegionId {
    rt.material
        .persist_derived_region(DerivedRegion {
            derived_region_id: DerivedRegionId::new(),
            subject_id: subject,
            derived_representation_id: representation,
            coordinate_kind: "text_span".into(),
            coordinate: serde_json::json!({"start":start,"end":end}),
            coordinate_hash: format!("{start}:{end}"),
            parent_derived_region_id: None,
            created_at: rt.cognition.now(subject),
        })
        .await
        .unwrap()
        .derived_region_id
}
