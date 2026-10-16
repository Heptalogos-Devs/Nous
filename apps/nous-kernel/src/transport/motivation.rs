// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_core::*;
use nous_motivation::*;
pub(super) fn scope(v: p::MotivationScope) -> Result<MotivationScope> {
    Ok(MotivationScope {
        description: v.description,
        entities: v
            .entities
            .into_iter()
            .map(EntityRef::new)
            .collect::<Result<_>>()?,
        group: v.group.map(EntityRef::new).transpose()?,
        channel: v.channel,
        conversation: v.conversation,
        work_context: v
            .work_context
            .map(|v| id(&v).map(WorkContextId))
            .transpose()?,
        domain: v.domain,
    })
}
fn scope_proto(v: MotivationScope) -> p::MotivationScope {
    p::MotivationScope {
        description: v.description,
        entities: v.entities.into_iter().map(|v| v.as_str().into()).collect(),
        group: v.group.map(|v| v.as_str().into()),
        channel: v.channel,
        conversation: v.conversation,
        work_context: v.work_context.map(|v| v.0.to_string()),
        domain: v.domain,
    }
}
fn enum_text<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .expect("enum")
        .as_str()
        .expect("enum string")
        .into()
}
pub(super) fn record(v: MotivationRecord) -> p::MotivationRecord {
    p::MotivationRecord {
        reference: Some(to_ref(v.reference())),
        object_ref: Some(to_ref(v.object_reference())),
        subject_id: v.subject_id.0.to_string(),
        current_revision_id: v.current_revision_id.to_string(),
        object_epoch: v.object_epoch,
        revision_no: v.revision_no,
        parent_revision_id: v.parent_revision_id.map(|id| id.to_string()),
        reason: v.reason,
        content: Some(content_proto(v.content)),
        acceptance_state: v.acceptance_state,
        integrity_state: v.integrity_state,
        suppression_state: v.suppression_state,
        created_at: Some(timestamp(v.created_at)),
        formed_at: Some(stamp_proto(v.formed_stamp)),
        recorded_at: Some(timestamp(v.recorded_at)),
        evidence: v
            .evidence
            .into_iter()
            .map(|e| p::MotivationEvidence {
                role: e.role,
                basis: Some(basis_proto(e.basis)),
                recorded_at: Some(timestamp(e.recorded_at)),
            })
            .collect(),
    }
}
fn motivation_ref(input: Option<p::CognitiveRef>, exact: bool) -> Result<CognitiveRef> {
    let r = from_ref(required(input, "Motivation reference")?)?;
    if r.domain() != CognitiveDomain::Motivation || r.is_cognition_revision() != exact {
        return Err(Error::Invalid(
            "Motivation reference has wrong family or precision".into(),
        ));
    }
    Ok(r)
}
#[tonic::async_trait]
impl p::motivation_service_server::MotivationService for KernelService {
    async fn select(
        &self,
        r: Request<p::MotivationSelectionRequest>,
    ) -> std::result::Result<Response<p::MotivationSelectionResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let owned = Selection::new(r)?;
            let result = self
                .0
                .require_motivation()?
                .select(owned.subject, owned.input())
                .await?;
            Ok(selection_proto(result))
        })
        .await
    }
    async fn evaluate(
        &self,
        r: Request<p::EvaluateConditionRequest>,
    ) -> std::result::Result<Response<p::ConditionEvaluation>, Status> {
        rpc_reply(async {
            let v = r.into_inner();
            let input = EvaluateCondition {
                subject: SubjectId(id(&v.subject_id)?),
                operation_id: OperationId(id(&v.operation_id)?),
                condition_revision: motivation_ref(v.condition_revision, true)?,
                scope: scope(v.scope.unwrap_or_default())?,
                source_read: read_scope(v.source_read)?,
                evidence_refs: v
                    .evidence_refs
                    .into_iter()
                    .map(from_ref)
                    .collect::<Result<_>>()?,
            };
            let reader = OfficialCriterionReader {
                material: self.0.material.clone(),
                social: self.0.social_cognition.clone(),
            };
            Ok(evaluation_proto(
                self.0
                    .require_motivation()?
                    .evaluate(input, &reader)
                    .await?,
            ))
        })
        .await
    }
    async fn list_evaluations(
        &self,
        r: Request<p::ConditionEvaluationListRequest>,
    ) -> std::result::Result<Response<p::ConditionEvaluationListResponse>, Status> {
        rpc_reply(async {
            let v = r.into_inner();
            let subject = SubjectId(id(&v.subject_id)?);
            let reference = motivation_ref(v.condition_revision, true)?;
            let scope = format!("motivation-evaluations:{}:{}", subject.0, reference);
            let (limit, after) = page(v.page, &scope)?;
            let (items, more) = self
                .0
                .require_motivation()?
                .evaluations(subject, reference, after, limit as usize)
                .await?;
            let next = if more {
                items
                    .last()
                    .map(|v| next_token(&scope, v.evaluation_id))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            Ok(p::ConditionEvaluationListResponse {
                items: items.into_iter().map(evaluation_proto).collect(),
                next_page_token: next,
            })
        })
        .await
    }
    async fn get_evaluation(
        &self,
        r: Request<p::ObjectRequest>,
    ) -> std::result::Result<Response<p::ConditionEvaluation>, Status> {
        rpc_reply(async {
            let v = r.into_inner();
            Ok(evaluation_proto(
                self.0
                    .require_motivation()?
                    .evaluation(SubjectId(id(&v.subject_id)?), id(&v.id)?)
                    .await?,
            ))
        })
        .await
    }
    async fn adopt(
        &self,
        r: Request<p::AdoptMotivationRequest>,
    ) -> std::result::Result<Response<p::MotivationRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_motivation()?
                    .adopt(AdoptMotivation {
                        subject: SubjectId(id(&r.subject_id)?),
                        operation_id: OperationId(id(&r.operation_id)?),
                        content: content(required(r.content, "Motivation content")?)?,
                        reason: r.reason,
                    })
                    .await?,
            ))
        })
        .await
    }
    async fn revise(
        &self,
        r: Request<p::ReviseMotivationRequest>,
    ) -> std::result::Result<Response<p::MotivationRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let reference = motivation_ref(r.target_revision, true)?;
            let current = self
                .0
                .require_motivation()?
                .get(subject, reference.uuid().expect("Motivation UUID"), true)
                .await?;
            if current.reference() != reference {
                return Err(Error::Invalid(
                    "Motivation reference family mismatch".into(),
                ));
            }
            Ok(record(
                self.0
                    .require_motivation()?
                    .revise(ReviseMotivation {
                        subject,
                        operation_id: OperationId(id(&r.operation_id)?),
                        object_id: current.object_id,
                        expected_revision: current.revision_id,
                        expected_epoch: r.expected_object_epoch,
                        content: content(required(r.content, "Motivation content")?)?,
                        reason: r.reason,
                    })
                    .await?,
            ))
        })
        .await
    }
    async fn add_evidence(
        &self,
        r: Request<p::AddMotivationEvidenceRequest>,
    ) -> std::result::Result<Response<p::MotivationRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_motivation()?
                    .add_evidence(
                        SubjectId(id(&r.subject_id)?),
                        OperationId(id(&r.operation_id)?),
                        motivation_ref(r.target_revision, true)?,
                        r.expected_object_epoch,
                        &r.role,
                        basis(required(r.basis, "Motivation basis")?)?,
                    )
                    .await?,
            ))
        })
        .await
    }
    async fn get(
        &self,
        r: Request<p::ObjectRequest>,
    ) -> std::result::Result<Response<p::MotivationRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_motivation()?
                    .get(SubjectId(id(&r.subject_id)?), id(&r.id)?, false)
                    .await?,
            ))
        })
        .await
    }
    async fn get_revision(
        &self,
        r: Request<p::ObjectRequest>,
    ) -> std::result::Result<Response<p::MotivationRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_motivation()?
                    .get(SubjectId(id(&r.subject_id)?), id(&r.id)?, true)
                    .await?,
            ))
        })
        .await
    }
    async fn list(
        &self,
        r: Request<p::MotivationListRequest>,
    ) -> std::result::Result<Response<p::MotivationListResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let scope = format!("motivation:{}:{}", subject.0, r.family);
            let (limit, last) = page(r.page, &scope)?;
            let (items, more) = self
                .0
                .require_motivation()?
                .list(subject, &r.family, last, limit as usize)
                .await?;
            let next_page_token = if more {
                next_token(&scope, items.last().expect("nonempty page").object_id)
            } else {
                String::new()
            };
            Ok(p::MotivationListResponse {
                items: items.into_iter().map(record).collect(),
                next_page_token,
            })
        })
        .await
    }
    async fn list_revisions(
        &self,
        r: Request<p::MotivationHistoryRequest>,
    ) -> std::result::Result<Response<p::MotivationListResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let reference = motivation_ref(r.object_ref, false)?;
            let object = reference.uuid().expect("Motivation UUID");
            if self
                .0
                .require_motivation()?
                .get(subject, object, false)
                .await?
                .object_reference()
                != reference
            {
                return Err(Error::Invalid(
                    "Motivation reference family mismatch".into(),
                ));
            }
            let scope = format!("motivation-history:{}:{}", subject.0, object);
            let (limit, last) = page(r.page, &scope)?;
            let (items, more) = self
                .0
                .require_motivation()?
                .history(subject, object, last, limit as usize)
                .await?;
            let next_page_token = if more {
                next_token(&scope, items.last().expect("nonempty page").revision_id)
            } else {
                String::new()
            };
            Ok(p::MotivationListResponse {
                items: items.into_iter().map(record).collect(),
                next_page_token,
            })
        })
        .await
    }
    async fn set_lifecycle(
        &self,
        r: Request<p::MotivationLifecycleRequest>,
    ) -> std::result::Result<Response<p::MotivationRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let m = required(r.mutation, "Motivation mutation")?;
            Ok(record(
                self.0
                    .require_motivation()?
                    .lifecycle(
                        SubjectId(id(&m.subject_id)?),
                        OperationId(id(&m.operation_id)?),
                        motivation_ref(m.object_ref, false)?,
                        m.expected_object_epoch,
                        &r.action,
                    )
                    .await?,
            ))
        })
        .await
    }
    async fn purge(
        &self,
        r: Request<p::MotivationMutationRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            self.0
                .require_motivation()?
                .purge(
                    SubjectId(id(&r.subject_id)?),
                    OperationId(id(&r.operation_id)?),
                    motivation_ref(r.object_ref, false)?,
                    r.expected_object_epoch,
                )
                .await
        })
        .await
    }
}

struct Selection {
    subject: SubjectId,
    scope: MotivationScope,
    sources: SourceReadScope,
    max_items: usize,
    max_bytes: usize,
    after: Option<uuid::Uuid>,
    at: Option<nous_core::DomainStamp>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
    history: bool,
}
impl Selection {
    fn new(v: p::MotivationSelectionRequest) -> Result<Self> {
        Ok(Self {
            subject: SubjectId(id(&v.subject_id)?),
            scope: scope(v.scope.unwrap_or_default())?,
            sources: v
                .source_read
                .map(|s| {
                    Ok::<_, Error>(SourceReadScope {
                        origin_conversation_refs: s.origin_conversation_refs,
                        subject_id: Some(SubjectId(id(&s.subject_id)?)),
                        source_refs: s
                            .source_refs
                            .into_iter()
                            .map(from_ref)
                            .collect::<Result<_>>()?,
                    })
                })
                .transpose()?
                .unwrap_or_default(),
            max_items: v.max_items as usize,
            max_bytes: v.max_bytes as usize,
            after: v.after.map(|v| id(&v)).transpose()?,
            at: v.at.map(domain_stamp).transpose()?,
            as_of: time(v.as_of)?,
            history: v.history,
        })
    }
    fn input(&self) -> MotivationSelectionRequest<'_> {
        MotivationSelectionRequest {
            scope: &self.scope,
            source_read: &self.sources,
            max_items: self.max_items,
            max_bytes: self.max_bytes,
            after: self.after,
            at: self.at.clone(),
            as_of: self.as_of,
            history: self.history,
        }
    }
}
fn selection_proto(v: MotivationSelection) -> p::MotivationSelectionResponse {
    p::MotivationSelectionResponse {
        items: v.records.into_iter().map(record).collect(),
        next: v.next.map(|v| v.to_string()),
        omitted: v.omitted,
    }
}

pub(super) fn read_scope(v: Option<p::SourceReadScope>) -> Result<SourceReadScope> {
    let v = required(v, "source_read")?;
    Ok(SourceReadScope {
        origin_conversation_refs: v.origin_conversation_refs,
        subject_id: Some(SubjectId(id(&v.subject_id)?)),
        source_refs: v
            .source_refs
            .into_iter()
            .map(from_ref)
            .collect::<Result<_>>()?,
    })
}
pub(super) fn stamp_proto(v: nous_core::DomainStamp) -> p::DomainStamp {
    p::DomainStamp {
        time_domain_ref: v.time_domain_ref,
        at: Some(timestamp(v.at)),
        recorded_at: Some(timestamp(v.recorded_at)),
    }
}
fn extent_proto(v: nous_core::DomainTemporalExtent) -> p::DomainTemporalExtent {
    p::DomainTemporalExtent {
        time_domain_ref: v.time_domain_ref,
        extent: Some(temporal_proto(&v.extent)),
    }
}
fn content(v: p::MotivationContent) -> Result<MotivationContent> {
    let m = MotivationMatter {
        matter_key: v.matter_key,
        statement: v.statement,
        scope: scope(required(v.scope, "scope")?)?,
        basis: v.basis.into_iter().map(basis).collect::<Result<_>>()?,
        valid_time_domain_ref: domain_temporal(v.valid_time.clone())?.time_domain_ref,
        valid_time: domain_temporal(v.valid_time)?.extent,
        producer: v.producer.map(from_producer).transpose()?,
        links: v
            .links
            .into_iter()
            .map(|l| {
                Ok(MotivationLink {
                    relation: l.relation,
                    target_revision: from_ref(required(l.target_revision, "target revision")?)?,
                })
            })
            .collect::<Result<_>>()?,
        condition: v
            .condition
            .map(|c| {
                let w = required(c.window, "condition window")?;
                Ok::<_, Error>(DesiredCondition {
                    expression: criterion(required(c.expression, "expression")?)?,
                    temporal_form: enum_value(&c.temporal_form)?,
                    window: nous_core::DomainTemporalExtent {
                        time_domain_ref: w.time_domain_ref,
                        extent: temporal(w.extent)?,
                    },
                    coverage_requirement: c.coverage_requirement,
                    hold_seconds: c.hold_seconds,
                })
            })
            .transpose()?,
    };
    match v.family.as_str() {
        "desire" => Ok(MotivationContent::Desire(m)),
        "concern" => Ok(MotivationContent::Concern(m)),
        "desired_condition" => Ok(MotivationContent::DesiredCondition(m)),
        _ => Err(Error::Invalid("unknown Motivation family".into())),
    }
}
fn content_proto(v: MotivationContent) -> p::MotivationContent {
    let family = v.family().into();
    let m = match v {
        MotivationContent::Desire(m)
        | MotivationContent::Concern(m)
        | MotivationContent::DesiredCondition(m) => m,
    };
    p::MotivationContent {
        family,
        matter_key: m.matter_key,
        statement: m.statement,
        scope: Some(scope_proto(m.scope)),
        basis: m.basis.into_iter().map(basis_proto).collect(),
        valid_time: Some(domain_temporal_proto(
            &m.valid_time_domain_ref,
            &m.valid_time,
        )),
        producer: m.producer.map(super::evidence::producer),
        links: m
            .links
            .into_iter()
            .map(|l| p::MotivationLink {
                relation: l.relation,
                target_revision: Some(to_ref(l.target_revision)),
            })
            .collect(),
        condition: m.condition.map(|c| p::DesiredConditionContent {
            expression: Some(criterion_proto(c.expression)),
            temporal_form: enum_text(c.temporal_form),
            window: Some(extent_proto(c.window)),
            coverage_requirement: c.coverage_requirement,
            hold_seconds: c.hold_seconds,
        }),
    }
}
fn criterion(v: p::CriterionExpression) -> Result<Criterion> {
    use p::criterion_expression::Expression as E;
    Ok(match required(v.expression, "criterion")? {
        E::EventWitness(v) => Criterion::EventWitness {
            version: v.version,
            event_type: v.event_type,
            target: v.target,
            activity: v.activity,
        },
        E::OwnerState(v) => Criterion::OwnerState {
            version: v.version,
            reference: from_ref(required(v.reference, "owner ref")?)?,
            state: v.state,
        },
        E::Measurement(v) => Criterion::Measurement {
            version: v.version,
            name: v.name,
            unit: v.unit,
            comparison: enum_value(&v.comparison)?,
            value: v.value,
        },
        E::Relation(v) => Criterion::Relation {
            version: v.version,
            reference: from_ref(required(v.reference, "relation ref")?)?,
            relation_type: v.relation_type,
            degree: v.degree,
        },
        E::Semantic(v) => Criterion::Semantic {
            version: v.version,
            question: v.question,
            material_refs: v
                .material_refs
                .into_iter()
                .map(from_ref)
                .collect::<Result<_>>()?,
        },
        E::All(v) => Criterion::All {
            items: v.items.into_iter().map(criterion).collect::<Result<_>>()?,
        },
        E::Any(v) => Criterion::Any {
            items: v.items.into_iter().map(criterion).collect::<Result<_>>()?,
        },
        E::Not(v) => Criterion::Not {
            item: Box::new(criterion(*v)?),
        },
    })
}
fn criterion_proto(v: Criterion) -> p::CriterionExpression {
    use p::criterion_expression::Expression as E;
    p::CriterionExpression {
        expression: Some(match v {
            Criterion::EventWitness {
                version,
                event_type,
                target,
                activity,
            } => E::EventWitness(p::EventWitnessCriterion {
                version,
                event_type,
                target,
                activity,
            }),
            Criterion::OwnerState {
                version,
                reference,
                state,
            } => E::OwnerState(p::OwnerStateCriterion {
                version,
                reference: Some(to_ref(reference)),
                state,
            }),
            Criterion::Measurement {
                version,
                name,
                unit,
                comparison,
                value,
            } => E::Measurement(p::MeasurementCriterion {
                version,
                name,
                unit,
                comparison: enum_text(comparison),
                value,
            }),
            Criterion::Relation {
                version,
                reference,
                relation_type,
                degree,
            } => E::Relation(p::RelationCriterion {
                version,
                reference: Some(to_ref(reference)),
                relation_type,
                degree,
            }),
            Criterion::Semantic {
                version,
                question,
                material_refs,
            } => E::Semantic(p::SemanticCriterion {
                version,
                question,
                material_refs: material_refs.iter().cloned().map(to_ref).collect(),
            }),
            Criterion::All { items } => E::All(p::CriterionOperands {
                items: items.into_iter().map(criterion_proto).collect(),
            }),
            Criterion::Any { items } => E::Any(p::CriterionOperands {
                items: items.into_iter().map(criterion_proto).collect(),
            }),
            Criterion::Not { item } => E::Not(Box::new(criterion_proto(*item))),
        }),
    }
}
pub(super) fn evaluation_proto(v: ConditionEvaluation) -> p::ConditionEvaluation {
    p::ConditionEvaluation {
        evaluation_id: v.evaluation_id.to_string(),
        condition_revision: Some(to_ref(v.condition_revision)),
        applicable: v.applicable,
        verdict: enum_text(v.verdict),
        final_conclusion: v.final_conclusion,
        evaluated_interval: Some(extent_proto(v.evaluated_interval)),
        coverage: v.coverage,
        evaluator_identity: v.evaluator_identity,
        inputs: v.inputs.iter().cloned().map(to_ref).collect(),
        leaves: v
            .leaves
            .into_iter()
            .map(|l| p::CriterionRead {
                evaluator: l.evaluator,
                complete: l.complete,
                method: l.method,
                producer: l.producer.map(super::evidence::producer),
                samples: l
                    .samples
                    .into_iter()
                    .map(|s| p::CriterionSample {
                        at: Some(timestamp(s.at)),
                        truth: enum_text(s.truth),
                        evidence: s.evidence.iter().cloned().map(to_ref).collect(),
                    })
                    .collect(),
            })
            .collect(),
        evaluated: Some(stamp_proto(v.evaluated)),
        explanation: v.explanation,
    }
}

impl KernelService {
    pub(super) async fn authorized_query_motivation_refs(
        &self,
        query: &p::QueryRequest,
    ) -> Result<(Vec<CognitiveRef>, bool)> {
        let subject = SubjectId(id(&query.subject_id)?);
        let Some(usage) = &query.usage else {
            return Ok((Vec::new(), false));
        };
        let Some(read) = &usage.source_read else {
            return Ok((Vec::new(), false));
        };
        if !self
            .0
            .subjects
            .subject(subject)
            .await?
            .capabilities
            .motivation
        {
            return Ok((Vec::new(), false));
        }
        let read = SourceReadScope {
            origin_conversation_refs: read.origin_conversation_refs.clone(),
            subject_id: Some(SubjectId(id(&read.subject_id)?)),
            source_refs: read
                .source_refs
                .clone()
                .into_iter()
                .map(from_ref)
                .collect::<Result<_>>()?,
        };
        let audience = usage.audience.as_ref();
        let scope = MotivationScope {
            description: "Trusted Host query invocation".into(),
            entities: usage
                .participants
                .iter()
                .map(|p| p.entity_ref.clone())
                .chain(
                    audience
                        .into_iter()
                        .flat_map(|a| a.recipient_entity_refs.clone()),
                )
                .map(EntityRef::new)
                .collect::<Result<_>>()?,
            group: audience
                .and_then(|a| a.group_entity_ref.clone())
                .map(EntityRef::new)
                .transpose()?,
            channel: audience.and_then(|a| a.channel_ref.clone()),
            conversation: audience.map(|a| a.conversation_ref.clone()),
            domain: usage.social_domain.clone(),
            work_context: query
                .work_context_id
                .as_deref()
                .map(id)
                .transpose()?
                .map(WorkContextId),
        };
        let modifiers = query.expression.as_ref().and_then(|e| e.modifiers.as_ref());
        let selected = self
            .0
            .require_motivation()?
            .select(
                subject,
                MotivationSelectionRequest {
                    scope: &scope,
                    source_read: &read,
                    max_items: 2048,
                    max_bytes: 4 * 1024 * 1024,
                    after: None,
                    at: modifiers
                        .and_then(|m| m.clock_now.clone())
                        .map(domain_stamp)
                        .transpose()?,
                    as_of: time(modifiers.and_then(|m| m.as_of))?,
                    history: modifiers.is_some_and(|m| m.history),
                },
            )
            .await?;
        Ok((
            selected.records.iter().map(|r| r.reference()).collect(),
            selected.omitted,
        ))
    }
}
