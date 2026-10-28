use super::{models::Semantic, *};
use nous_domain::media::MediaReference;
use std::collections::BTreeMap;

pub(super) fn group(
    unit: &ImportTextUnit,
    semantic: &Semantic,
) -> Result<Option<(StructuredProposalGroup, Vec<StructuredTargetSlot>)>> {
    let mut names = semantic.entities.clone();
    for triple in &semantic.relations {
        names.push(triple.subject.clone());
        names.push(triple.object.clone());
    }
    let quote = unit.effective_strategy == ImportTextStrategy::Quote;
    if quote {
        names.push(format!("引文 {} #{}", unit.input_source_id, unit.ordinal));
    }
    // Names only bind endpoints within this one immutable extraction, never across units.
    let mut targets = BTreeMap::new();
    for name in names {
        let name = name.trim().to_owned();
        validate_name(&name, "extracted entity label")?;
        targets.entry(name).or_insert_with(CognitiveId::new);
    }
    let source = MediaReference::SourceRegion {
        id: unit.source_region_id.clone(),
    };
    let mut evidence = vec![EvidenceLink {
        source: EvidenceSource::Media {
            reference: source.clone(),
        },
        polarity: EvidencePolarity::Support,
        relation: None,
        explanation: String::new(),
    }];
    for id in &unit.context_region_ids {
        if id == &unit.source_region_id {
            continue;
        }
        evidence.push(EvidenceLink {
            source: EvidenceSource::Media {
                reference: MediaReference::SourceRegion { id: id.clone() },
            },
            polarity: EvidencePolarity::Context,
            relation: None,
            explanation: String::new(),
        });
    }
    let mut slots = Vec::new();
    let mut changes = Vec::new();
    for (name, id) in &targets {
        let key = format!("e{}", slots.len());
        slots.push(StructuredTargetSlot {
            key: key.clone(),
            kind: CognitiveKind::Entity,
            id: id.clone(),
            expected_revision: 0,
        });
        changes.push(StructuredProposalChange {
            slot_key: key,
            body: CognitiveContent::Entity(EntityContent {
                entity_type: EntityType::Other,
                label: name.clone(),
                aliases: vec![],
                description: String::new(),
                identifiers: vec![],
                mentions: vec![EntityMention {
                    target: SemanticTarget::Exact {
                        reference: RetrievalTarget::Media(source.clone()),
                    },
                    role: if quote { "verbatim" } else { "mentioned" }.into(),
                }],
                attributes: vec![],
            }),
            evidence: evidence.clone(),
        });
    }
    for triple in &semantic.relations {
        let from = targets
            .get(triple.subject.trim())
            .ok_or_else(|| Error::internal("extracted subject missing"))?;
        let to = targets
            .get(triple.object.trim())
            .ok_or_else(|| Error::internal("extracted object missing"))?;
        if from == to {
            return Err(Error::invalid(
                "extracted relation needs distinct cognitive endpoints",
            ));
        }
        let key = format!("r{}", slots.len());
        slots.push(StructuredTargetSlot {
            key: key.clone(),
            kind: CognitiveKind::Association,
            id: CognitiveId::new(),
            expected_revision: 0,
        });
        changes.push(StructuredProposalChange {
            slot_key: key,
            body: CognitiveContent::Association(AssociationContent {
                from: CognitiveRef::structured(CognitiveKind::Entity, from, 1),
                to: CognitiveRef::structured(CognitiveKind::Entity, to, 1),
                relation: triple.predicate.clone(),
                polarity: AssociationPolarity::Positive,
                basis_class: AssociationBasisClass::SourceEvidence,
                description: String::new(),
            }),
            evidence: evidence.clone(),
        });
    }
    if changes.is_empty() {
        Ok(None)
    } else {
        Ok(Some((
            StructuredProposalGroup {
                key: "unit".into(),
                depends_on: vec![],
                changes,
            },
            slots,
        )))
    }
}

pub(super) fn temporal(
    value: &crate::import_models::TimeExtraction,
) -> Result<(Option<TimePoint>, Option<TimeRange>)> {
    let point = |value: &str| -> Result<TimePoint> {
        let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|error| {
            Error::invalid("time extraction is not an explicit RFC3339 offset timestamp")
                .with_cause(&error)
        })?;
        Ok(TimePoint {
            domain: "unix_ms".into(),
            coordinate: parsed.timestamp_millis(),
        })
    };
    let event = value.event_time.as_deref().map(point).transpose()?;
    let start = value
        .event_time_start
        .as_deref()
        .or_else(|| value.time_range.as_ref().map(|range| range[0].as_str()))
        .map(point)
        .transpose()?;
    let end = value
        .event_time_end
        .as_deref()
        .or_else(|| value.time_range.as_ref().map(|range| range[1].as_str()))
        .map(point)
        .transpose()?;
    let event_range = if start.is_some() || end.is_some() {
        let event_range = TimeRange {
            domain: "unix_ms".into(),
            start: start.map(|value| value.coordinate),
            end: end.map(|value| value.coordinate),
        };
        event_range.validate()?;
        Some(event_range)
    } else {
        None
    };
    Ok((event, event_range))
}
