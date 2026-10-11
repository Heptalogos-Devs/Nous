// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::*;

impl KernelService {
    pub(super) async fn query(
        &self,
        bound: nous_runtime::BoundQuery,
        lease: nous_runtime::QueryLease,
        pool_limit: Option<usize>,
    ) -> Result<k::KernelQueryResponse> {
        lease.require_live()?;
        let execution = Box::pin(self.0.execute_bound_query(bound, pool_limit)).await?;
        lease.require_live()?;
        let inspection = inspect_bound_query(&execution.bound)?;
        let feedback_bound = execution.bound.clone();
        let requires_finalization =
            pool_limit.is_some() || !execution.result.resource_actions.is_empty();
        let (result, ticket) = if requires_finalization {
            let (result, ticket) = self.0.cognition.retain_query(execution, lease)?;
            (result, ticket.map(|value| value.to_string()))
        } else {
            (execution.result, None)
        };
        if requires_finalization && ticket.is_none() {
            self.0
                .cognition
                .record_query_feedback(&feedback_bound, &result)
                .await?;
        }
        let mut response = query_response(result);
        response.bound_query = Some(inspection);
        Ok(k::KernelQueryResponse {
            response: Some(response),
            validation_ticket: ticket,
        })
    }
    pub(super) async fn finalize_query(
        &self,
        input: k::FinalizeQueryRequest,
    ) -> Result<p::QueryResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let order = input
            .order
            .into_iter()
            .map(|item| {
                Ok((
                    from_ref(required(item.reference, "reference")?)?,
                    item.score,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let result = self
            .0
            .cognition
            .finalize_query(
                subject,
                id(&input.validation_ticket)?,
                order,
                input
                    .external_results
                    .into_iter()
                    .map(|value| {
                        Ok(ExternalResourceResult {
                            action_id: id(&value.action_id)?,
                            resource_ref: ResourceRef::new(value.resource_ref)?,
                            status: value.status,
                            records: value
                                .records
                                .into_iter()
                                .map(from_resource_record)
                                .collect::<Result<Vec<_>>>()?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
                nous_runtime::CognitiveContributors {
                    material: Some(&self.0.material),
                    shared: None,
                    memory: self
                        .0
                        .memory
                        .as_ref()
                        .map(|owner| owner as &dyn nous_runtime::CognitiveContributor),
                },
            )
            .await?;
        Ok(query_response(result))
    }
}

fn query_response(result: CognitiveQueryResult) -> p::QueryResponse {
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
            .map(|value| p::ResourceAction {
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
            })
            .collect(),
        degradation: result
            .degradation
            .into_iter()
            .map(|value| p::Degradation {
                code: value.code,
                detail: value.detail.unwrap_or_default(),
            })
            .collect(),
        bound_query: None,
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
fn from_resource_record(value: p::ExternalResourceRecord) -> Result<ExternalResourceRecord> {
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
        exploration: enum_or_default(&modifiers.exploration)?,
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
        effort: enum_or_default(&modifiers.effort)?,
        capabilities: if let Some(capabilities) = input.capabilities {
            CapabilityPolicy {
                text_embedding: enum_or_default(&capabilities.text_embedding)?,
                multimodal_interpretation: enum_or_default(
                    &capabilities.multimodal_interpretation,
                )?,
                residual_sensing: enum_or_default(&capabilities.residual_sensing)?,
                rerank: enum_or_default(&capabilities.rerank)?,
                query_concept_enrichment: enum_or_default(&capabilities.query_concept_enrichment)?,
            }
        } else {
            CapabilityPolicy::default()
        },
        diagnostics: enum_or_default(&modifiers.diagnostics)?,
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
        return Err(Error::Invalid("query expression depth exceeded".into()));
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
        return Err(Error::Invalid("execution controls are root-only".into()));
    }
    let mut node = CognitiveQueryExpr {
        operation: match expression.operation.as_str() {
            "atom" => QueryOperation::Atom,
            "all" => QueryOperation::All,
            "any" => QueryOperation::Any,
            _ => return Err(Error::Invalid("unknown query expression operation".into())),
        },
        ..Default::default()
    };
    for cue in expression.cues {
        append_cue(&mut node, cue)?;
    }
    for preference in modifiers.preferences {
        let operand = if let Some(cue) = preference.cue {
            if !preference.key.is_empty() {
                return Err(Error::Invalid("preference must have one operand".into()));
            }
            let mut value = CognitiveQueryExpr::default();
            append_cue(&mut value, cue)?;
            if let Some(cue) = value.cues.pop() {
                PreferenceOperand::Cue(cue)
            } else if let Some(QueryTarget::Exact { reference }) = value.targets.pop() {
                PreferenceOperand::Exact(reference)
            } else {
                return Err(Error::Invalid("empty preference cue".into()));
            }
        } else {
            PreferenceOperand::Recent(match preference.key.as_str() {
                "recent:occurred" => TimeAxis::Occurred,
                "recent:observed" => TimeAxis::Observed,
                "recent:valid" => TimeAxis::Valid,
                "recent:formed" => TimeAxis::Formed,
                "recent:recorded" => TimeAxis::Recorded,
                _ => {
                    return Err(Error::Invalid(
                        "recent requires an explicit time axis".into(),
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
    match cue
        .cue
        .ok_or_else(|| Error::Invalid("cue is empty".into()))?
    {
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
        formed_at: value
            .freshness
            .formed_at
            .map(|v| timestamp(v).seconds.to_string()),
        recorded_at: value
            .freshness
            .recorded_at
            .map(|v| timestamp(v).seconds.to_string()),
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
    }
}

pub(super) fn inspect_bound_query(bound: &nous_runtime::BoundQuery) -> Result<String> {
    let mut seeds = bound.topology_seed_refs.clone();
    seeds.extend(
        bound
            .runtime_refs
            .iter()
            .cloned()
            .map(|reference| (reference, "runtime_situation".into())),
    );
    seeds.extend(
        bound
            .exact_bindings
            .iter()
            .map(|binding| (binding.bound_ref.clone(), "exact_target".into())),
    );
    seeds.extend(
        bound
            .representation
            .source_refs
            .iter()
            .filter(|(reference, source)| {
                source.starts_with("explicit")
                    && matches!(reference, CognitiveRef::Tag(_) | CognitiveRef::Entity(_))
            })
            .cloned(),
    );
    seeds.sort_by_key(|(reference, source)| (reference.to_string(), source.clone()));
    seeds.dedup();
    serde_json::to_string(&serde_json::json!({
        "prepared_query":bound.source_query, "query_id":bound.query_id,
        "representation":bound.representation, "context_snapshot":bound.context_snapshot.as_ref(), "current_refs":bound.runtime_refs,
        "query_activation":bound.activation, "concept_enrichment":bound.concept_enrichment,
        "query_concept_requirement":bound.source_query.capabilities.query_concept_enrichment,
        "historical_view":bound.historical_authority.as_ref().map(|view|serde_json::json!({"digest":view.snapshot_digest,"as_of":view.as_of,"revision_view":view.revision_view})),
        "temporal_frame":bound.source_query.temporal_frame, "result_projection":bound.source_query.projection,
        "topology_seeds":seeds, "exact_bindings":bound.exact_bindings,
        "profile_id":bound.retrieval_policy.cognitive_profile.id(),
        "profile_source":bound.config_snapshot.source(nous_runtime::COGNITIVE_PROFILE.path()),
        "config_snapshot_digest":bound.config_snapshot.effective_digest,
        "authority_watermark":bound.bound_at_authority_seq,
        "embedding_digest":bound.representation.sha256,
        "embedding_space":bound.selected_embedding_space,
    }))
    .map_err(|error| Error::Infrastructure(error.to_string()))
}

fn enum_or_default<T: serde::de::DeserializeOwned + Default>(input: &str) -> Result<T> {
    if input.is_empty() {
        Ok(T::default())
    } else {
        enum_value(input)
    }
}
