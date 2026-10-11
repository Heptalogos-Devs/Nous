// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::*;

pub(super) fn compile_query(
    input: p::QueryRequest,
    default_limit: usize,
) -> Result<CognitiveQuery> {
    let expression = required(input.expression, "expression")?;
    let modifiers = expression.modifiers.clone().unwrap_or_default();
    let query = CognitiveQuery {
        projection: match modifiers.projection.as_ref() {
            Some(projection) => ResultProjection {
                domains: projection
                    .domains
                    .iter()
                    .map(|domain| enum_value(domain))
                    .collect::<Result<_>>()?,
            },
            None => ResultProjection::default(),
        },
        temporal_frame: TemporalFrame {
            clock_now: time(modifiers.clock_now)?
                .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH),
            authority_view: time(modifiers.as_of)?
                .map_or(AuthorityView::Current, AuthorityView::AsOf),
            original_expressions: modifiers.temporal_expressions.clone(),
            revision_view: if modifiers.history {
                RevisionView::History
            } else {
                RevisionView::Current
            },
        },

        work_context: input.work_context_id.as_deref().map(id).transpose()?,
        subject: SubjectId(id(&input.subject_id)?),
        session: input
            .session_id
            .as_deref()
            .map(id)
            .transpose()?
            .map(SessionId),
        situation: if let Some(situation) = input.situation {
            SituationDescriptor {
                consumer: situation.consumer,
                current_refs: situation
                    .current_refs
                    .into_iter()
                    .map(from_ref)
                    .collect::<Result<_>>()?,
                current_objects: situation
                    .current_objects
                    .into_iter()
                    .map(ObjectRef::new)
                    .collect::<Result<_>>()?,
                object_descriptions: situation.object_descriptions.into_iter().collect(),
            }
        } else {
            SituationDescriptor::default()
        },
        expression: compile_expression(expression, true, 0)?,
        exploration: super::query_projection::enum_or_default(&modifiers.exploration)?,
        resources: ResourceIntent {
            synopsis_only: false,
        },
        result_need: ResultNeed {
            limit: modifiers
                .limit
                .map_or(default_limit, |limit| limit as usize),
            need_evidence: true,
            need_materialization_handles: modifiers.materialize,
        },
        effort: super::query_projection::enum_or_default(&modifiers.effort)?,
        capabilities: if let Some(capabilities) = input.capabilities {
            CapabilityPolicy {
                text_embedding: super::query_projection::enum_or_default(
                    &capabilities.text_embedding,
                )?,
                multimodal_interpretation: super::query_projection::enum_or_default(
                    &capabilities.multimodal_interpretation,
                )?,
                residual_sensing: super::query_projection::enum_or_default(
                    &capabilities.residual_sensing,
                )?,
                rerank: super::query_projection::enum_or_default(&capabilities.rerank)?,
                query_concept_enrichment: super::query_projection::enum_or_default(
                    &capabilities.query_concept_enrichment,
                )?,
            }
        } else {
            CapabilityPolicy::default()
        },
        diagnostics: super::query_projection::enum_or_default(&modifiers.diagnostics)?,
    };
    query.validate()?;
    Ok(query)
}

fn compile_expression(
    expression: p::QueryExpr,
    root: bool,
    depth: usize,
) -> Result<CognitiveQueryExpr> {
    if depth > 16 {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "query expression depth exceeded",
        ));
    }
    let modifiers = expression.modifiers.unwrap_or_default();
    if !root
        && (!modifiers.effort.is_empty()
            || modifiers.limit.is_some()
            || !modifiers.diagnostics.is_empty()
            || !modifiers.exploration.is_empty()
            || modifiers.materialize
            || modifiers.projection.is_some()
            || modifiers.as_of.is_some()
            || modifiers.history
            || modifiers.clock_now.is_some()
            || !modifiers.temporal_expressions.is_empty())
    {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "execution controls are root-only",
        ));
    }
    let mut node = CognitiveQueryExpr {
        operation: match expression.operation.as_str() {
            "atom" => QueryOperation::Atom,
            "all" => QueryOperation::All,
            "any" => QueryOperation::Any,
            _ => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Runtime,
                    nous_core::ErrorCode::InvalidRequest,
                    "unknown query expression operation",
                ));
            }
        },
        ..Default::default()
    };
    for cue in expression.cues {
        append_cue(&mut node, cue)?;
    }
    for target in expression.targets {
        let target = match target.target.ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "query target is empty",
            )
        })? {
            p::query_target::Target::EntityNeighborhood(value) => QueryTarget::EntityNeighborhood {
                entity_ref: EntityRef::new(value)?,
            },
            p::query_target::Target::SchemaNeighborhood(value) => QueryTarget::SchemaNeighborhood {
                schema: CognitiveSchemaId(id(&value)?),
            },
            p::query_target::Target::Exact(reference) => QueryTarget::Exact {
                reference: from_ref(reference)?,
            },
        };
        node.targets.push(target);
    }
    for preference in modifiers.preferences {
        let operand = if let Some(cue) = preference.cue {
            if !preference.key.is_empty() {
                return Err(Error::new(
                    nous_core::ErrorOwner::Runtime,
                    nous_core::ErrorCode::InvalidRequest,
                    "preference must have one operand",
                ));
            }
            let mut value = CognitiveQueryExpr::default();
            append_cue(&mut value, cue)?;
            if let Some(cue) = value.cues.pop() {
                PreferenceOperand::Cue(cue)
            } else if let Some(QueryTarget::Exact { reference }) = value.targets.pop() {
                PreferenceOperand::Exact(reference)
            } else {
                return Err(Error::new(
                    nous_core::ErrorOwner::Runtime,
                    nous_core::ErrorCode::InvalidRequest,
                    "empty preference cue",
                ));
            }
        } else {
            PreferenceOperand::Recent(match preference.key.as_str() {
                "recent:occurred" => TimeAxis::Occurred,
                "recent:observed" => TimeAxis::Observed,
                "recent:valid" => TimeAxis::Valid,
                "recent:formed" => TimeAxis::Formed,
                "recent:recorded" => TimeAxis::Recorded,
                _ => {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Runtime,
                        nous_core::ErrorCode::InvalidRequest,
                        "Unsupported recent preference; expected recent:occurred, recent:observed, recent:valid, recent:formed, or recent:recorded",
                    ));
                }
            })
        };
        node.preferences.push(QueryPreference {
            negative: preference.negative,
            operand,
        });
    }
    if let Some(constraints) = modifiers.constraints {
        node.constraints = constraints_from_proto(constraints)?;
    }
    node.constraints.current_authority = if modifiers.current_authority.is_empty() {
        CurrentAuthorityNeed::None
    } else {
        enum_value(&modifiers.current_authority)?
    };
    node.children = expression
        .children
        .into_iter()
        .map(|child| compile_expression(child, false, depth + 1))
        .collect::<Result<_>>()?;
    Ok(node)
}

fn append_cue(query: &mut CognitiveQueryExpr, cue: p::Cue) -> Result<()> {
    match cue.cue.ok_or_else(|| {
        Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "cue is empty",
        )
    })? {
        p::cue::Cue::Text(value) => query.cues.push(Cue::Text(TextCue { text: value })),
        p::cue::Cue::Concept(value) => query.cues.push(Cue::Concept(ConceptCue { text: value })),
        p::cue::Cue::SchemaId(value) => query.cues.push(Cue::Schema(SchemaCue {
            schema: CognitiveSchemaId(id(&value)?),
        })),
        p::cue::Cue::EntityRef(value) => query.cues.push(Cue::Entity(EntityCue {
            entity_ref: EntityRef::new(value)?,
        })),
        p::cue::Cue::TagId(value) => query.cues.push(Cue::Tag(TagCue {
            tag: TagId(id(&value)?),
        })),
        p::cue::Cue::ResourceRef(value) => query.cues.push(Cue::Resource(ResourceCue {
            resource: ResourceRef::new(value)?,
        })),
        p::cue::Cue::ExternalObjectRef(value) => query.cues.push(Cue::Object(ObjectCue {
            object_ref: ObjectRef::new(value)?,
        })),
        p::cue::Cue::ArtifactId(value) => query.cues.push(Cue::Artifact(ArtifactCue {
            artifact: ArtifactId(id(&value)?),
        })),
        p::cue::Cue::MediaRegion(value) => query.cues.push(Cue::MediaRegion(MediaRegionCue {
            region: from_ref(value)?,
        })),
        p::cue::Cue::Relation(value) => query.cues.push(Cue::Relation(RelationCue {
            from: from_ref(required(value.from, "relation source")?)?,
            to: from_ref(required(value.to, "relation target")?)?,
        })),
        p::cue::Cue::Example(value) => query.cues.push(Cue::Example(ExampleCue { text: value })),
        p::cue::Cue::Reference(value) => {
            let reference = from_ref(value)?;
            query.targets.push(QueryTarget::Exact { reference });
        }
    }
    Ok(())
}

fn predicate(value: Option<p::TimePredicate>) -> Result<Option<TimePredicate>> {
    value
        .map(|value| {
            let predicate = match required(value.predicate, "time predicate")? {
                p::time_predicate::Predicate::Point(at) => TimePredicate::Point {
                    at: required(time(Some(at))?, "time point")?,
                },
                p::time_predicate::Predicate::Range(range) => TimePredicate::Range {
                    start: time(range.start)?,
                    end: time(range.end)?,
                },
            };
            predicate.validate()?;
            Ok(predicate)
        })
        .transpose()
}

fn constraints_from_proto(value: p::QueryConstraints) -> Result<QueryConstraints> {
    Ok(QueryConstraints {
        current_authority: CurrentAuthorityNeed::None,
        source_classes_include: value
            .source_classes_include
            .into_iter()
            .map(SourceClass::from)
            .collect(),
        source_classes_exclude: value
            .source_classes_exclude
            .into_iter()
            .map(SourceClass::from)
            .collect(),
        cognitive_roles_include: value.cognitive_roles,
        formation_modes_include: value.formation_modes,
        entity_requirements: value
            .entity_requirements
            .into_iter()
            .map(EntityRef::new)
            .collect::<Result<_>>()?,
        occurred: predicate(value.occurred)?,
        observed: predicate(value.observed)?,
        valid: predicate(value.valid)?,
        formed: predicate(value.formed)?,
        recorded: predicate(value.recorded)?,
        include_suppressed: value.include_suppressed,
        authority: value.authority.map(|v| enum_value(&v)).transpose()?,
        modalities: value
            .modalities
            .into_iter()
            .map(|v| enum_value(&v))
            .collect::<Result<_>>()?,
        evidence_classes: value.evidence_classes,
    })
}
