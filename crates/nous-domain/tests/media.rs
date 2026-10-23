use nous_domain::{media::*, *};
use std::collections::BTreeSet;

fn digest() -> String {
    "a".repeat(64)
}
fn interpretation(source: MediaReference) -> MediaInterpretation {
    MediaInterpretation {
        summary: MediaSummary {
            content: "a reported scene".into(),
            basis_refs: vec![source.clone()],
        },
        coverage: InterpretationCoverage {
            visual: CoverageStatus::Reported,
            audio: CoverageStatus::NotAvailable,
            embedded_text: CoverageStatus::NotAvailable,
            source_text: CoverageStatus::NotAvailable,
        },
        observations: vec![MediaObservation {
            kind: MediaObservationKind::Scene,
            content: "a person is described near a door".into(),
            evidence_channel: EvidenceChannel::Visual,
            basis: InterpretationBasis::Reported,
            certainty: InterpretationCertainty::Uncertain,
            start_ms: None,
            end_ms: None,
            basis_refs: vec![source],
        }],
        mentions: vec![],
        embedded_text: vec![],
        source_text: vec![],
        speech: vec![],
        interpretations: vec![],
        uncertainties: vec![],
    }
}
fn representation(source: MediaReference) -> DerivedRepresentation {
    DerivedRepresentation {
        id: DerivedRepresentationId::new(),
        namespace_id: NamespaceId::new(),
        subject_id: SubjectId::new(),
        revision: 1,
        inputs: vec![DerivationInput {
            ordinal: 0,
            source: source.clone(),
            role: DerivationInputRole::Description,
        }],
        strategy: DerivationStrategy::DescribeThenStructure,
        kind: RepresentationKind::StructuredInterpretation,
        payload: RepresentationPayload::Interpretation {
            value: interpretation(source),
        },
        producer: DerivationProducer {
            signature: digest(),
            implementation: "structured-media".into(),
            implementation_revision: "412b062082afb75ccfae4c3f69fc606d3e3cc862".into(),
            model_call_id: Some(ModelCallId::new()),
            prompt_digest: Some(digest()),
            schema_digest: Some(digest()),
            config_digest: digest(),
            preprocessing_digest: None,
        },
        access: ChannelAccess {
            visual: ChannelMode::Representation,
            audio: ChannelMode::Unavailable,
            source_text: ChannelMode::Unavailable,
        },
        observation_id: None,
        formed_at: TimePoint {
            domain: "utc".into(),
            coordinate: 1,
        },
        recorded_at: "2026-10-23T00:00:00Z".into(),
        commit_seq: 1,
        supersedes: None,
        quality: DerivationQuality::default(),
    }
}
#[test]
fn streaming_original_expectations_and_typed_region_bounds_preserve_identity() -> Result<()> {
    let namespace = NamespaceId::new();
    let material = Material {
        id: MaterialId::new(),
        namespace_id: namespace.clone(),
        content_hash: digest(),
        media_type: "image/png".into(),
        byte_len: 100,
        source: Some("source:image".into()),
        event_time: None,
        recorded_at: "2026-10-23T00:00:00Z".into(),
        commit_seq: 1,
    };
    let request = PutMediaRequest {
        namespace_id: namespace,
        media_type: material.media_type.clone(),
        declared_byte_len: Some(100),
        expected_content_hash: Some(digest()),
        source: None,
        event_time: None,
    };
    request.validate_received(100, &digest())?;
    assert!(request.validate_received(99, &digest()).is_err());
    let metadata = MediaMetadata::Image {
        width: 640,
        height: 480,
        frames: 1,
        duration_ms: None,
    };
    let parent = SourceRegion {
        id: SourceRegionId::new(),
        namespace_id: material.namespace_id.clone(),
        material_id: material.id.clone(),
        parent: None,
        coordinate: SourceCoordinate::ImageRect {
            rect: PixelRect {
                x: 10,
                y: 10,
                width: 100,
                height: 100,
            },
        },
        coordinate_hash: digest(),
        recorded_at: material.recorded_at.clone(),
        commit_seq: 2,
    };
    parent.validate_against(&material, &metadata, None)?;
    let mut child = SourceRegion {
        id: SourceRegionId::new(),
        parent: Some(parent.id.clone()),
        coordinate: SourceCoordinate::ImageRect {
            rect: PixelRect {
                x: 20,
                y: 20,
                width: 20,
                height: 20,
            },
        },
        ..parent.clone()
    };
    child.validate_against(&material, &metadata, Some(&parent))?;
    assert!(
        child
            .source_references()
            .contains(&MediaReference::Original {
                material_id: material.id.clone()
            })
    );
    child.coordinate = SourceCoordinate::ImageRect {
        rect: PixelRect {
            x: 600,
            y: 400,
            width: 20,
            height: 20,
        },
    };
    assert!(
        child
            .validate_against(&material, &metadata, Some(&parent))
            .is_err()
    );
    Ok(())
}
#[test]
fn committed_description_cannot_be_upgraded_to_direct_media_observation() -> Result<()> {
    let mut direct = representation(MediaReference::Original {
        material_id: MaterialId::new(),
    });
    direct.inputs[0].role = DerivationInputRole::Source;
    direct.strategy = DerivationStrategy::DirectStructured;
    direct.access.visual = ChannelMode::Original;
    if let RepresentationPayload::Interpretation { value } = &mut direct.payload {
        value.coverage.visual = CoverageStatus::Observed;
        value.observations[0].basis = InterpretationBasis::Direct;
    }
    direct.validate(None)?;
    direct.strategy = DerivationStrategy::DescriptionOnly;
    direct.kind = RepresentationKind::ImageDescription;
    direct.payload = RepresentationPayload::Text {
        text: "a description from original pixels".into(),
    };
    direct.validate(None)?;
    let source = MediaReference::Representation {
        reference: RepresentationRef {
            id: DerivedRepresentationId::new(),
            revision: 1,
        },
    };
    let mut value = representation(source);
    value.validate(None)?;
    let key = value.derivation_key_material()?;
    value.producer.model_call_id = Some(ModelCallId::new());
    value.recorded_at = "2026-10-24T00:00:00Z".into();
    assert_eq!(key, value.derivation_key_material()?);
    if let RepresentationPayload::Interpretation { value } = &mut value.payload {
        value.observations[0].basis = InterpretationBasis::Direct;
    }
    assert!(value.validate(None).is_err());
    if let RepresentationPayload::Interpretation { value } = &mut value.payload {
        value.observations[0].basis = InterpretationBasis::Reported;
        value.summary.basis_refs = vec![MediaReference::Original {
            material_id: MaterialId::new(),
        }];
    }
    assert!(value.validate(None).is_err());
    Ok(())
}
#[test]
fn interpretation_locators_and_video_sampling_preserve_exact_source_limits() -> Result<()> {
    let source = MediaReference::Representation {
        reference: RepresentationRef {
            id: DerivedRepresentationId::new(),
            revision: 2,
        },
    };
    let value = representation(source);
    value.validate(None)?;
    let root = MaterialId::new();
    let mut contact = Observation {
        id: ObservationId::new(),
        namespace_id: value.namespace_id.clone(),
        subject_id: value.subject_id.clone(),
        material_id: root.clone(),
        observed_at: None,
        recorded_at: value.recorded_at.clone(),
        commit_seq: 1,
        kind: ObservationKind::Import,
    };
    let exposure = MediaExposure {
        observation_id: contact.id.clone(),
        namespace_id: contact.namespace_id.clone(),
        subject_id: contact.subject_id.clone(),
        locator: MediaReference::Representation {
            reference: value.reference(),
        },
        access: value.access,
        recorded_at: value.recorded_at.clone(),
        commit_seq: 2,
    };
    let roots = BTreeSet::from([root]);
    assert!(
        exposure
            .validate_against(&contact, &roots, value.access)
            .is_err()
    );
    contact.kind = ObservationKind::Contact;
    exposure.validate_against(&contact, &roots, value.access)?;
    assert!(
        exposure
            .validate_against(
                &contact,
                &roots,
                ChannelAccess {
                    visual: ChannelMode::Original,
                    ..value.access
                }
            )
            .is_err()
    );
    let mut region = DerivedRegion {
        id: DerivedRegionId::new(),
        namespace_id: value.namespace_id.clone(),
        subject_id: value.subject_id.clone(),
        representation: value.reference(),
        parent: None,
        selector: DerivedSelector::Interpretation {
            item: InterpretationSelector::Observation { index: 0 },
        },
        recorded_at: value.recorded_at.clone(),
        commit_seq: 2,
    };
    region.validate_against(&value, None)?;
    region.selector = DerivedSelector::Interpretation {
        item: InterpretationSelector::Observation { index: 1 },
    };
    assert!(region.validate_against(&value, None).is_err());
    let mut quality = DerivationQuality {
        partial: true,
        source_duration_ms: Some(1000),
        sampling: Some(FrameSamplingReport {
            implementation: "ffmpeg".into(),
            implementation_version: "actual-probe-version".into(),
            policy: "uniform bounded sampling".into(),
            sampled_duration_ms: 900,
            requested_timestamps_ms: vec![0, 400, 800],
            sampled_timestamps_ms: vec![0, 805],
            omitted_timestamps_ms: vec![400],
            audio_included: false,
            audio_omitted: true,
        }),
    };
    quality.validate(Some(1000))?;
    quality.partial = false;
    assert!(quality.validate(Some(1000)).is_err());
    assert_eq!(
        ChannelAccess {
            visual: ChannelMode::Original,
            audio: ChannelMode::Original,
            source_text: ChannelMode::Unavailable
        }
        .represented(),
        ChannelAccess {
            visual: ChannelMode::Representation,
            audio: ChannelMode::Representation,
            source_text: ChannelMode::Unavailable
        }
    );
    Ok(())
}
