// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::*;

pub(super) fn query_response(
    result: CognitiveQueryResult,
    bound_query: Option<p::QueryInspection>,
) -> p::QueryResponse {
    p::QueryResponse {
        query_id: result.query_id.to_string(),
        status: enum_name(result.status),
        hits: result
            .results
            .into_iter()
            .enumerate()
            .map(|(rank, value)| hit(value, rank + 1))
            .collect(),
        resource_actions: result
            .resource_actions
            .into_iter()
            .map(resource_action)
            .collect(),
        degradation: result
            .degradation
            .into_iter()
            .map(|value| p::Degradation {
                code: value.code,
                detail: value.detail.unwrap_or_default(),
            })
            .collect(),
        bound_query,
        diagnostics: result.diagnostics.map(|value| p::QueryDiagnostics {
            candidate_counts: value
                .candidate_counts
                .into_iter()
                .map(|(key, value)| (key, value as u64))
                .collect(),
            lane_status: value.lane_status.into_iter().collect(),
            trace: value
                .trace
                .map(|trace| trace.to_string())
                .unwrap_or_default(),
            topology_complete: value.topology_complete,
            topology_discarded_mass: value.topology_discarded_mass,
        }),
        resource_records: result
            .resource_records
            .into_iter()
            .map(to_resource_record)
            .collect(),
    }
}

pub(super) fn resource_action(value: nous_core::ResourceActionSuggestion) -> p::ResourceAction {
    p::ResourceAction {
        resource_ref: value.resource.as_str().into(),
        action: value.action,
        reason: value.reason,
        current_authority: value.current_authority,
        action_id: value.action_id.to_string(),
        query_text: value.query_text,
        limit: value.limit as u32,
        materialize: value.materialize,
        adapter_kind: value.adapter_kind,
        provider_profile: value.provider_profile,
        provider_locator: value.provider_locator,
        descriptor_digest: value.descriptor_digest,
    }
}

fn to_resource_record(value: ExternalResourceRecord) -> p::ExternalResourceRecord {
    let reference = value.reference;
    p::ExternalResourceRecord {
        resource_ref: value.resource_ref.as_str().into(),
        reference: Some(p::StableExternalRef {
            provider_kind: reference.provider_kind,
            provider_profile: reference.provider_profile,
            profile_digest: reference.profile_digest,
            resource_ref: reference.resource_ref.as_str().into(),
            provider_resource_id: reference.provider_resource_id,
            entry_id: reference.entry_id,
            entry_version: reference.entry_version,
            content_digest: reference.content_digest,
            source_locator: reference.source_locator,
            retrieved_at: reference.retrieved_at,
            access_scope: reference.access_scope,
        }),
        title: value.title,
        content: value.content,
        provider_rank: value.provider_rank,
        provider_score: value.provider_score,
        version_status: value.version_status,
        access_status: value.access_status,
    }
}
pub(super) fn from_resource_record(
    value: p::ExternalResourceRecord,
) -> Result<ExternalResourceRecord> {
    let reference = required(value.reference, "external reference")?;
    Ok(ExternalResourceRecord {
        resource_ref: ResourceRef::new(value.resource_ref)?,
        reference: StableExternalRef {
            provider_kind: reference.provider_kind,
            provider_profile: reference.provider_profile,
            profile_digest: reference.profile_digest,
            resource_ref: ResourceRef::new(reference.resource_ref)?,
            provider_resource_id: reference.provider_resource_id,
            entry_id: reference.entry_id,
            entry_version: reference.entry_version,
            content_digest: reference.content_digest,
            source_locator: reference.source_locator,
            retrieved_at: reference.retrieved_at,
            access_scope: reference.access_scope,
        },
        title: value.title,
        content: value.content,
        provider_rank: value.provider_rank,
        provider_score: value.provider_score,
        version_status: value.version_status,
        access_status: value.access_status,
    })
}
fn hit(value: CognitiveHit, rank: usize) -> p::Hit {
    p::Hit {
        reference: Some(to_ref(value.reference)),
        revision: value.revision.map(to_ref),
        text: value.representation,
        authority: enum_name(value.authority),
        evidence: value
            .evidence
            .into_iter()
            .map(|e| p::Evidence {
                epistemic_relation: e
                    .epistemic_relation
                    .map(|relation| relation.as_str().into()),
                reference: Some(to_ref(e.reference)),
                basis_role: e.basis_role,
            })
            .collect(),
        evidence_families: value
            .match_evidence
            .families
            .into_iter()
            .map(enum_name)
            .collect(),
        entity_refs: value
            .entity_refs
            .into_iter()
            .map(|v| v.as_str().into())
            .collect(),
        lexical_ref: None,
        semantic_role: value.semantic_role,
        cognitive_role: value.cognitive_role,
        formation_mode: value.formation_mode,
        score: Some(p::HitScore {
            baseline: value.match_evidence.base_rank_score,
            preference: value.match_evidence.preference_score,
            rerank: value.match_evidence.rerank_score,
            r#final: value.match_evidence.final_score,
            baseline_rank: value.match_evidence.baseline_rank,
            final_rank: rank as u32,
            best_lane_rank: value.match_evidence.best_lane_rank,
            variants: value.match_evidence.variants,
        }),
        authority_epoch: value.authority_epoch,
        freshness: Some(p::HitFreshnessDescriptor {
            occurred: value
                .freshness
                .occurred
                .into_iter()
                .map(temporal_extent)
                .collect(),
            observed_at: value.freshness.observed_at.map(timestamp),
            valid_time: Some(temporal_extent(value.freshness.valid_time)),
            formed_at: value.freshness.formed_at.map(timestamp),
            recorded_at: value.freshness.recorded_at.map(timestamp),
        }),
    }
}

fn query_expr(
    expression: &CognitiveQueryExpr,
    root: bool,
    query: &CognitiveQuery,
) -> Result<p::QueryExpr> {
    use nous_core::{PreferenceOperand, QueryOperation, QueryTarget};
    let mut modifiers = p::QueryModifiers {
        constraints: Some(query_constraints(&expression.constraints)),
        current_authority: enum_name(expression.constraints.current_authority),
        preferences: expression
            .preferences
            .iter()
            .map(|preference| match &preference.operand {
                PreferenceOperand::Cue(cue) => Ok(p::Preference {
                    negative: preference.negative,
                    cue: Some(cue_proto(cue.clone())),
                    key: String::new(),
                }),
                PreferenceOperand::Exact(reference) => Ok(p::Preference {
                    negative: preference.negative,
                    cue: Some(p::Cue {
                        cue: Some(p::cue::Cue::Reference(to_ref(reference.clone()))),
                    }),
                    key: String::new(),
                }),
                PreferenceOperand::Recent(axis) => Ok(p::Preference {
                    negative: preference.negative,
                    cue: None,
                    key: format!("recent:{}", enum_name(*axis)),
                }),
            })
            .collect::<Result<Vec<_>>>()?,
        ..Default::default()
    };
    if root {
        modifiers.projection = Some(p::ResultProjection {
            domains: query
                .projection
                .domains
                .iter()
                .map(|domain| domain.name().to_owned())
                .collect(),
        });
        modifiers.effort = enum_name(query.effort);
        modifiers.limit = Some(query.result_need.limit as u32);
        modifiers.exploration = enum_name(query.exploration);
        modifiers.materialize = query.result_need.need_materialization_handles;
        modifiers.diagnostics = enum_name(query.diagnostics);
        modifiers.as_of = match query.temporal_frame.authority_view {
            AuthorityView::Current => None,
            AuthorityView::AsOf(at) => Some(timestamp(at)),
        };
        modifiers.history = query.temporal_frame.revision_view == RevisionView::History;
        modifiers.clock_now = Some(timestamp(query.temporal_frame.clock_now));
        modifiers.temporal_expressions = query.temporal_frame.original_expressions.clone();
    }
    Ok(p::QueryExpr {
        operation: match expression.operation {
            QueryOperation::Atom => "atom",
            QueryOperation::All => "all",
            QueryOperation::Any => "any",
        }
        .into(),
        cues: expression.cues.iter().cloned().map(cue_proto).collect(),
        children: expression
            .children
            .iter()
            .map(|child| query_expr(child, false, query))
            .collect::<Result<_>>()?,
        modifiers: Some(modifiers),
        targets: expression
            .targets
            .iter()
            .map(|target| p::QueryTarget {
                target: Some(match target {
                    QueryTarget::EntityNeighborhood { entity_ref } => {
                        p::query_target::Target::EntityNeighborhood(entity_ref.as_str().to_owned())
                    }
                    QueryTarget::SchemaNeighborhood { schema } => {
                        p::query_target::Target::SchemaNeighborhood(schema.0.to_string())
                    }
                    QueryTarget::Exact { reference } => {
                        p::query_target::Target::Exact(to_ref(reference.clone()))
                    }
                }),
            })
            .collect(),
    })
}

fn cue_proto(cue: Cue) -> p::Cue {
    use p::cue::Cue as ProtoCue;
    let cue = match cue {
        Cue::Text(value) => ProtoCue::Text(value.text),
        Cue::Concept(value) => ProtoCue::Concept(value.text),
        Cue::Entity(value) => ProtoCue::EntityRef(value.entity_ref.as_str().to_owned()),
        Cue::Object(value) => ProtoCue::ExternalObjectRef(value.object_ref.as_str().to_owned()),
        Cue::Artifact(value) => ProtoCue::ArtifactId(value.artifact.0.to_string()),
        Cue::MediaRegion(value) => ProtoCue::MediaRegion(to_ref(value.region)),
        Cue::Tag(value) => ProtoCue::TagId(value.tag.0.to_string()),
        Cue::Schema(value) => ProtoCue::SchemaId(value.schema.0.to_string()),
        Cue::Relation(value) => ProtoCue::Relation(p::RelationCue {
            from: Some(to_ref(value.from)),
            to: Some(to_ref(value.to)),
        }),
        Cue::Example(value) => ProtoCue::Example(value.text),
        Cue::Resource(value) => ProtoCue::ResourceRef(value.resource.as_str().to_owned()),
    };
    p::Cue { cue: Some(cue) }
}

fn query_constraints(value: &QueryConstraints) -> p::QueryConstraints {
    p::QueryConstraints {
        source_classes_include: value
            .source_classes_include
            .iter()
            .map(|v| v.as_str().to_owned())
            .collect(),
        source_classes_exclude: value
            .source_classes_exclude
            .iter()
            .map(|v| v.as_str().to_owned())
            .collect(),
        cognitive_roles: value.cognitive_roles_include.clone(),
        formation_modes: value.formation_modes_include.clone(),
        entity_requirements: value
            .entity_requirements
            .iter()
            .map(|v| v.as_str().to_owned())
            .collect(),
        occurred: value.occurred.map(time_predicate),
        observed: value.observed.map(time_predicate),
        valid: value.valid.map(time_predicate),
        formed: value.formed.map(time_predicate),
        recorded: value.recorded.map(time_predicate),
        include_suppressed: value.include_suppressed,
        authority: value.authority.map(enum_name),
        modalities: value.modalities.iter().copied().map(enum_name).collect(),
        evidence_classes: value.evidence_classes.clone(),
    }
}

fn time_predicate(value: TimePredicate) -> p::TimePredicate {
    use p::time_predicate::Predicate;
    p::TimePredicate {
        predicate: Some(match value {
            TimePredicate::Point { at } => Predicate::Point(timestamp(at)),
            TimePredicate::Range { start, end } => Predicate::Range(p::TimeInterval {
                start: start.map(timestamp),
                end: end.map(timestamp),
            }),
        }),
    }
}

fn temporal_extent(value: TemporalExtent) -> p::TemporalExtent {
    use p::temporal_extent::Value;
    p::TemporalExtent {
        value: match value {
            TemporalExtent::Unknown => None,
            TemporalExtent::Instant { at } => Some(Value::Instant(timestamp(at))),
            TemporalExtent::Interval { start, end } => Some(Value::Interval(p::TimeInterval {
                start: start.map(timestamp),
                end: end.map(timestamp),
            })),
        },
    }
}

pub(super) fn query_temporal(value: &TemporalFrame) -> p::QueryTemporalFrame {
    use p::{QueryAuthorityViewKind as AuthorityKind, QueryRevisionViewKind as RevisionKind};
    let (authority_view, authority_as_of) = match value.authority_view {
        AuthorityView::Current => (AuthorityKind::Current, None),
        AuthorityView::AsOf(at) => (AuthorityKind::AsOf, Some(timestamp(at))),
    };
    p::QueryTemporalFrame {
        clock_now: Some(timestamp(value.clock_now)),
        authority_view: authority_view as i32,
        authority_as_of,
        revision_view: match value.revision_view {
            RevisionView::Current => RevisionKind::Current as i32,
            RevisionView::History => RevisionKind::History as i32,
        },
        original_expressions: value.original_expressions.clone(),
    }
}

pub(super) fn query_representation(
    representation: &nous_runtime::QueryRepresentation,
) -> p::QueryRepresentation {
    p::QueryRepresentation {
        version: representation.representation_version.clone(),
        text: representation.text.clone(),
        sha256: representation.sha256.clone(),
        source_refs: representation
            .source_refs
            .iter()
            .map(|(reference, source)| p::QuerySourceRef {
                reference: Some(to_ref(reference.clone())),
                source: source.clone(),
            })
            .collect(),
        truncation_flags: representation.truncation_flags.clone(),
    }
}

pub(super) fn inspect_bound_query_with_plan(
    bound: &nous_runtime::BoundQuery,
    plan: &nous_runtime::QueryPlan,
) -> Result<p::QueryInspection> {
    use nous_runtime::AuthorityReadView;
    let temporal = query_temporal(&bound.source_query.temporal_frame);
    let (view_kind, historical_view_digest) = match &bound.read_context.view {
        AuthorityReadView::Current { .. } => (p::QueryAuthorityViewKind::Current as i32, None),
        AuthorityReadView::Historical(snapshot) => (
            p::QueryAuthorityViewKind::AsOf as i32,
            Some(snapshot.snapshot_digest.clone()),
        ),
    };
    let bound_authority_seq = match &bound.read_context.view {
        AuthorityReadView::Current {
            bound_authority_seq,
        } => *bound_authority_seq,
        AuthorityReadView::Historical(_) => bound.bound_at_authority_seq,
    };
    let context = &bound.context_snapshot;
    let representation = &bound.representation;
    let activation = &bound.activation;
    let query = &bound.source_query;
    let readout_policy_digest = match bound.retrieval_policy.cognitive_profile {
        nous_runtime::CognitiveProfile::BaselineRrf => bound.config_snapshot.digest_for(&[])?,
        nous_runtime::CognitiveProfile::NousNodePotential => bound
            .config_snapshot
            .digest_for(nous_retrieval::WAVE_READOUT_CONFIG_KEYS)?,
        nous_runtime::CognitiveProfile::VcpDtsc | nous_runtime::CognitiveProfile::VcpRiverMemo => {
            bound
                .config_snapshot
                .digest_for(&[nous_retrieval::VCP_READOUT.path()])?
        }
    };
    Ok(p::QueryInspection {
        query_id: bound.query_id.to_string(),
        canonical_query: Some(query_expr(&query.expression, true, query)?),
        representation: Some(query_representation(representation)),
        context: Some(p::QueryContextSnapshot {
            session: context.session.as_ref().map(|session| p::Session {
                session_id: session.session_id.0.to_string(),
                subject_id: session.subject_id.0.to_string(),
                runtime_revision: session.runtime_revision,
                closed: session.closed_at.is_some(),
                resident_refs: session
                    .resident
                    .iter()
                    .map(|item| to_ref(item.reference.clone()))
                    .collect(),
                active_work_context_id: session.active_work_context_id.map(|id| id.to_string()),
            }),
            work_context: context.work_context.clone().map(super::work_context::view),
            situation_refs: context.situation_refs.iter().cloned().map(to_ref).collect(),
            source_refs: context
                .source_refs
                .iter()
                .map(|(reference, source)| p::QuerySourceRef {
                    reference: Some(to_ref(reference.clone())),
                    source: source.clone(),
                })
                .collect(),
            history_exclusions: context
                .history_exclusions
                .iter()
                .map(|d| p::Degradation {
                    code: d.code.clone(),
                    detail: d.detail.clone().unwrap_or_default(),
                })
                .collect(),
            digest: context.digest.clone(),
        }),
        read_context: Some(p::ReadContext {
            subject_id: bound.read_context.subject.0.to_string(),
            view: view_kind,
            historical_view_digest,
            temporal: Some(temporal.clone()),
            accessibility: Some(p::AccessibilityQueryPolicy {
                effort: enum_name(bound.read_context.accessibility.effort),
                exact_target_bypasses_auto_level: bound
                    .read_context
                    .accessibility
                    .exact_target_bypasses_auto_level,
            }),
            bound_authority_seq,
            purge_epoch: bound.read_context.purge_epoch,
        }),
        exact_bindings: bound
            .exact_bindings
            .iter()
            .map(|binding| p::QueryExactBinding {
                requested_ref: Some(to_ref(binding.requested_ref.clone())),
                bound_ref: Some(to_ref(binding.bound_ref.clone())),
                bound_object_epoch: binding.bound_object_epoch,
                mutable_object: binding.mutable_object,
            })
            .collect(),
        activation: Some(p::QueryActivationInspection {
            frozen: activation.frozen,
            concept_catalog: activation
                .concept_catalog
                .iter()
                .map(|candidate| p::QueryConceptCandidate {
                    key: candidate.key.clone(),
                    tag: Some(to_ref(CognitiveRef::Tag(candidate.tag))),
                    semantic_text: candidate.semantic_text.clone(),
                    strength: candidate.strength,
                })
                .collect(),
            explicit_tags: activation
                .explicit_tags
                .iter()
                .map(|tag| p::QueryTagActivation {
                    tag: Some(to_ref(CognitiveRef::Tag(tag.tag))),
                    strength: tag.strength,
                    source: enum_name(tag.source),
                    origin: tag.origin.clone(),
                })
                .collect(),
            inferred_tags: activation
                .inferred_tags
                .iter()
                .map(|tag| p::QueryTagActivation {
                    tag: Some(to_ref(CognitiveRef::Tag(tag.tag))),
                    strength: tag.strength,
                    source: enum_name(tag.source),
                    origin: tag.origin.clone(),
                })
                .collect(),
            seeds: activation
                .seeds
                .iter()
                .map(|seed| p::QueryActivationSeed {
                    reference: Some(to_ref(seed.reference.clone())),
                    origin: seed.origin.clone(),
                    strength: seed.strength,
                })
                .collect(),
            runtime_refs: bound.runtime_refs.iter().cloned().map(to_ref).collect(),
            exploration: enum_name(activation.exploration),
            generation_id: activation
                .concept_generation
                .map(|v| v.0.to_string())
                .unwrap_or_default(),
            query_embedding_digest: activation
                .query_embedding_digest
                .clone()
                .unwrap_or_default(),
            degradation: activation
                .degradation
                .iter()
                .map(|d| p::Degradation {
                    code: d.code.clone(),
                    detail: d.detail.clone().unwrap_or_default(),
                })
                .collect(),
            model_calls: activation.model_calls as u32,
            model_completed: activation.model_completed,
        }),
        plan: Some(p::QueryPlanInspection {
            profile_id: bound.retrieval_policy.cognitive_profile.id().into(),
            profile_source: bound
                .config_snapshot
                .source(nous_runtime::COGNITIVE_PROFILE.path())
                .map(enum_name)
                .unwrap_or_default(),
            enabled_lanes: plan.enabled_lanes.iter().copied().map(enum_name).collect(),
            lane_budgets: plan
                .lane_budgets
                .iter()
                .map(|(lane, budget)| (enum_name(*lane), *budget as u32))
                .collect(),
            candidate_limit: plan.candidate_limit as u32,
            final_validation_budget: plan.final_validation_budget as u32,
            topology_required: bound.topology_required,
            readout_policy_digest,
        }),
        config_snapshot_digest: bound.config_snapshot.effective_digest.clone(),
        selected_embedding_space: bound.selected_embedding_space.as_ref().map(|space| {
            p::QueryEmbeddingSpaceView {
                space_hash: space.space_hash.clone(),
                model_identity: space.model_identity.clone(),
                dimension: space.dimension,
            }
        }),
        capabilities: Some(p::QueryCapabilities {
            text_embedding: enum_name(query.capabilities.text_embedding),
            multimodal_interpretation: enum_name(query.capabilities.multimodal_interpretation),
            residual_sensing: enum_name(query.capabilities.residual_sensing),
            rerank: enum_name(query.capabilities.rerank),
            query_concept_enrichment: enum_name(query.capabilities.query_concept_enrichment),
        }),
    })
}

pub(super) fn enum_or_default<T: serde::de::DeserializeOwned + Default>(input: &str) -> Result<T> {
    if input.is_empty() {
        Ok(T::default())
    } else {
        enum_value(input)
    }
}
