// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_core::*;
use nous_social::*;
fn scope(v: p::SocialScope) -> Result<SocialScope> {
    Ok(SocialScope {
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
fn scope_proto(v: SocialScope) -> p::SocialScope {
    p::SocialScope {
        description: v.description,
        entities: v.entities.into_iter().map(|v| v.as_str().into()).collect(),
        group: v.group.map(|v| v.as_str().into()),
        channel: v.channel,
        conversation: v.conversation,
        work_context: v.work_context.map(|v| v.0.to_string()),
        domain: v.domain,
    }
}
fn judgment(v: Option<p::SocialJudgment>) -> Result<SocialJudgment> {
    let v = required(v, "social judgment")?;
    Ok(SocialJudgment {
        state: v.state,
        explanation: v.explanation,
        estimate: v.estimate,
        estimate_method: v.estimate_method,
    })
}
fn judgment_proto(v: SocialJudgment) -> p::SocialJudgment {
    p::SocialJudgment {
        state: v.state,
        explanation: v.explanation,
        estimate: v.estimate,
        estimate_method: v.estimate_method,
    }
}
fn content(v: p::SocialContent) -> Result<SocialContent> {
    Ok(match required(v.content, "Social content")? {
        p::social_content::Content::Relation(v) => SocialContent::SocialRelation(RelationContent {
            from: EntityRef::new(v.from)?,
            to: EntityRef::new(v.to)?,
            relation_type: enum_value(&v.relation_type)?,
            role: v.role,
            degree: v
                .degree
                .map(|v| {
                    Ok::<_, Error>(match required(v.degree, "relation degree")? {
                        p::relation_degree::Degree::Ordinal(v) => RelationDegree::Ordinal(v),
                        p::relation_degree::Degree::Affinity(v) => RelationDegree::Affinity(v),
                        p::relation_degree::Degree::State(v) => RelationDegree::State(v),
                    })
                })
                .transpose()?,
            symmetric: v.symmetric,
            scope: scope(required(v.scope, "social scope")?)?,
            statement: v.statement,
            judgment: judgment(v.judgment)?,
            coordination_expectations: v.coordination_expectations,
            basis: v.basis.into_iter().map(basis).collect::<Result<_>>()?,
            valid_time: temporal(v.valid_time)?,
            producer: v.producer.map(from_producer).transpose()?,
        }),
        p::social_content::Content::Convention(v) => {
            SocialContent::LanguageConvention(ConventionContent {
                expression: v.expression,
                variants: v.variants,
                case_insensitive: v.case_insensitive,
                meaning: v.meaning,
                referent: v.referent.map(EntityRef::new).transpose()?,
                scope: scope(required(v.scope, "social scope")?)?,
                pragmatic_use: v.pragmatic_use,
                judgment: judgment(v.judgment)?,
                basis: v.basis.into_iter().map(basis).collect::<Result<_>>()?,
                valid_time: temporal(v.valid_time)?,
                producer: v.producer.map(from_producer).transpose()?,
            })
        }
    })
}
fn content_proto(v: SocialContent) -> p::SocialContent {
    p::SocialContent {
        content: Some(match v {
            SocialContent::SocialRelation(v) => {
                p::social_content::Content::Relation(p::SocialRelationContent {
                    from: v.from.as_str().into(),
                    to: v.to.as_str().into(),
                    relation_type: enum_text(v.relation_type),
                    role: v.role,
                    degree: v.degree.map(|v| p::RelationDegree {
                        degree: Some(match v {
                            RelationDegree::Ordinal(v) => p::relation_degree::Degree::Ordinal(v),
                            RelationDegree::Affinity(v) => p::relation_degree::Degree::Affinity(v),
                            RelationDegree::State(v) => p::relation_degree::Degree::State(v),
                        }),
                    }),
                    symmetric: v.symmetric,
                    scope: Some(scope_proto(v.scope)),
                    statement: v.statement,
                    judgment: Some(judgment_proto(v.judgment)),
                    coordination_expectations: v.coordination_expectations,
                    basis: v.basis.into_iter().map(basis_proto).collect(),
                    valid_time: Some(temporal_proto(&v.valid_time)),
                    producer: v.producer.map(super::evidence::producer),
                })
            }
            SocialContent::LanguageConvention(v) => {
                p::social_content::Content::Convention(p::LanguageConventionContent {
                    expression: v.expression,
                    variants: v.variants,
                    case_insensitive: v.case_insensitive,
                    meaning: v.meaning,
                    referent: v.referent.map(|v| v.as_str().into()),
                    scope: Some(scope_proto(v.scope)),
                    pragmatic_use: v.pragmatic_use,
                    judgment: Some(judgment_proto(v.judgment)),
                    basis: v.basis.into_iter().map(basis_proto).collect(),
                    valid_time: Some(temporal_proto(&v.valid_time)),
                    producer: v.producer.map(super::evidence::producer),
                })
            }
        }),
    }
}
fn enum_text<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .expect("enum")
        .as_str()
        .expect("enum string")
        .into()
}
pub(super) fn record(v: SocialRecord) -> p::SocialRecord {
    p::SocialRecord {
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
        formed_at: Some(formation_proto(
            &v.formed_time_domain_ref,
            v.formed_at,
            v.recorded_at,
        )),
        recorded_at: Some(timestamp(v.recorded_at)),
        evidence: v
            .evidence
            .into_iter()
            .map(|e| p::SocialEvidence {
                role: e.role,
                basis: Some(basis_proto(e.basis)),
                recorded_at: Some(timestamp(e.recorded_at)),
            })
            .collect(),
    }
}
fn social_ref(input: Option<p::CognitiveRef>, exact: bool) -> Result<CognitiveRef> {
    let r = from_ref(required(input, "Social reference")?)?;
    if r.domain() != CognitiveDomain::SocialCognition || r.is_cognition_revision() != exact {
        return Err(Error::Invalid(
            "Social reference has wrong family or precision".into(),
        ));
    }
    Ok(r)
}
#[tonic::async_trait]
impl p::social_service_server::SocialService for KernelService {
    async fn select(
        &self,
        r: Request<p::SocialSelectionRequest>,
    ) -> std::result::Result<Response<p::SocialSelectionResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let owned = Selection::new(r)?;
            let result = self
                .0
                .require_social()?
                .select(owned.subject, owned.input())
                .await?;
            Ok(selection_proto(result))
        })
        .await
    }
    async fn neighborhood(
        &self,
        r: Request<p::SocialSelectionRequest>,
    ) -> std::result::Result<Response<p::SocialSelectionResponse>, Status> {
        if r.get_ref().endpoint.is_none() {
            return Err(Status::invalid_argument("neighborhood requires endpoint"));
        }
        self.select(r).await
    }
    async fn pair(
        &self,
        r: Request<p::SocialPairRequest>,
    ) -> std::result::Result<Response<p::SocialPairResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let owned = Selection::new(required(r.selection, "Social selection")?)?;
            let result = self
                .0
                .require_social()?
                .pair(
                    owned.subject,
                    EntityRef::new(r.from)?,
                    EntityRef::new(r.to)?,
                    owned.input(),
                )
                .await?;
            let direction = |v: RelationDirection| p::RelationDirection {
                from: v.from.as_str().into(),
                to: v.to.as_str().into(),
                records: v.records.into_iter().map(record).collect(),
                unknown: v.unknown,
                inverse_views: v
                    .inverse_views
                    .into_iter()
                    .map(|v| p::RelationInverseView {
                        role: v.role,
                        record: Some(record(v.record)),
                    })
                    .collect(),
            };
            Ok(p::SocialPairResponse {
                forward: Some(direction(result.forward)),
                reverse: Some(direction(result.reverse)),
                omitted: result.omitted,
                next: result.next.map(|v| v.to_string()),
            })
        })
        .await
    }
    async fn resolve_conventions(
        &self,
        r: Request<p::ResolveConventionsRequest>,
    ) -> std::result::Result<Response<p::ResolveConventionsResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let owned = Selection::new(required(r.selection, "Social selection")?)?;
            let result = self
                .0
                .require_social()?
                .resolve_conventions(owned.subject, &r.original, &r.consumption, owned.input())
                .await?;
            Ok(p::ResolveConventionsResponse {
                original: result.original,
                consumption: result.consumption,
                candidates: result.candidates.into_iter().map(record).collect(),
                conflicts: result.conflicts,
                omitted: result.omitted,
                next: result.next.map(|v| v.to_string()),
                explanation: result.explanation,
            })
        })
        .await
    }
    async fn adopt(
        &self,
        r: Request<p::AdoptSocialRequest>,
    ) -> std::result::Result<Response<p::SocialRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_social()?
                    .adopt(AdoptSocial {
                        subject: SubjectId(id(&r.subject_id)?),
                        operation_id: OperationId(id(&r.operation_id)?),
                        content: content(required(r.content, "Social content")?)?,
                        reason: r.reason,
                    })
                    .await?,
            ))
        })
        .await
    }
    async fn revise(
        &self,
        r: Request<p::ReviseSocialRequest>,
    ) -> std::result::Result<Response<p::SocialRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let reference = social_ref(r.target_revision, true)?;
            let current = self
                .0
                .require_social()?
                .get(subject, reference.uuid().expect("Social UUID"), true)
                .await?;
            if current.reference() != reference {
                return Err(Error::Invalid("Social reference family mismatch".into()));
            }
            Ok(record(
                self.0
                    .require_social()?
                    .revise(ReviseSocial {
                        subject,
                        operation_id: OperationId(id(&r.operation_id)?),
                        object_id: current.object_id,
                        expected_revision: current.revision_id,
                        expected_epoch: r.expected_object_epoch,
                        content: content(required(r.content, "Social content")?)?,
                        reason: r.reason,
                    })
                    .await?,
            ))
        })
        .await
    }
    async fn add_evidence(
        &self,
        r: Request<p::AddSocialEvidenceRequest>,
    ) -> std::result::Result<Response<p::SocialRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_social()?
                    .add_evidence(
                        SubjectId(id(&r.subject_id)?),
                        OperationId(id(&r.operation_id)?),
                        social_ref(r.target_revision, true)?,
                        r.expected_object_epoch,
                        &r.role,
                        basis(required(r.basis, "Social basis")?)?,
                    )
                    .await?,
            ))
        })
        .await
    }
    async fn get(
        &self,
        r: Request<p::ObjectRequest>,
    ) -> std::result::Result<Response<p::SocialRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_social()?
                    .get(SubjectId(id(&r.subject_id)?), id(&r.id)?, false)
                    .await?,
            ))
        })
        .await
    }
    async fn get_revision(
        &self,
        r: Request<p::ObjectRequest>,
    ) -> std::result::Result<Response<p::SocialRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_social()?
                    .get(SubjectId(id(&r.subject_id)?), id(&r.id)?, true)
                    .await?,
            ))
        })
        .await
    }
    async fn list(
        &self,
        r: Request<p::SocialListRequest>,
    ) -> std::result::Result<Response<p::SocialListResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let scope = format!("social:{}:{}", subject.0, r.family);
            let (limit, last) = page(r.page, &scope)?;
            let (items, more) = self
                .0
                .require_social()?
                .list(subject, &r.family, last, limit as usize)
                .await?;
            let next_page_token = if more {
                next_token(&scope, items.last().expect("nonempty page").object_id)
            } else {
                String::new()
            };
            Ok(p::SocialListResponse {
                items: items.into_iter().map(record).collect(),
                next_page_token,
            })
        })
        .await
    }
    async fn list_revisions(
        &self,
        r: Request<p::SocialHistoryRequest>,
    ) -> std::result::Result<Response<p::SocialListResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let reference = social_ref(r.object_ref, false)?;
            let object = reference.uuid().expect("Social UUID");
            if self
                .0
                .require_social()?
                .get(subject, object, false)
                .await?
                .object_reference()
                != reference
            {
                return Err(Error::Invalid("Social reference family mismatch".into()));
            }
            let scope = format!("social-history:{}:{}", subject.0, object);
            let (limit, last) = page(r.page, &scope)?;
            let (items, more) = self
                .0
                .require_social()?
                .history(subject, object, last, limit as usize)
                .await?;
            let next_page_token = if more {
                next_token(&scope, items.last().expect("nonempty page").revision_id)
            } else {
                String::new()
            };
            Ok(p::SocialListResponse {
                items: items.into_iter().map(record).collect(),
                next_page_token,
            })
        })
        .await
    }
    async fn set_lifecycle(
        &self,
        r: Request<p::SocialLifecycleRequest>,
    ) -> std::result::Result<Response<p::SocialRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let m = required(r.mutation, "Social mutation")?;
            Ok(record(
                self.0
                    .require_social()?
                    .lifecycle(
                        SubjectId(id(&m.subject_id)?),
                        OperationId(id(&m.operation_id)?),
                        social_ref(m.object_ref, false)?,
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
        r: Request<p::SocialMutationRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            self.0
                .require_social()?
                .purge(
                    SubjectId(id(&r.subject_id)?),
                    OperationId(id(&r.operation_id)?),
                    social_ref(r.object_ref, false)?,
                    r.expected_object_epoch,
                )
                .await
        })
        .await
    }
}

struct Selection {
    subject: SubjectId,
    scope: SocialScope,
    sources: SourceReadScope,
    max_items: usize,
    max_bytes: usize,
    after: Option<uuid::Uuid>,
    expression: Option<String>,
    endpoint: Option<EntityRef>,
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
    history: bool,
}
impl Selection {
    fn new(v: p::SocialSelectionRequest) -> Result<Self> {
        Ok(Self {
            subject: SubjectId(id(&v.subject_id)?),
            scope: scope(v.scope.unwrap_or_default())?,
            sources: v
                .source_read
                .map(|s| {
                    Ok::<_, Error>(SourceReadScope {
                        origin_conversation_refs: s.origin_conversation_refs.clone(),
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
            expression: v.expression,
            endpoint: v.endpoint.map(EntityRef::new).transpose()?,
            at: time(v.at)?,
            as_of: time(v.as_of)?,
            history: v.history,
        })
    }
    fn input(&self) -> SocialSelectionRequest<'_> {
        SocialSelectionRequest {
            scope: &self.scope,
            source_read: &self.sources,
            max_items: self.max_items,
            max_bytes: self.max_bytes,
            after: self.after,
            expression: self.expression.as_deref(),
            endpoint: self.endpoint.as_ref(),
            counterpart: None,
            at: self.at,
            as_of: self.as_of,
            history: self.history,
        }
    }
}
fn selection_proto(v: SocialSelection) -> p::SocialSelectionResponse {
    p::SocialSelectionResponse {
        items: v.records.into_iter().map(record).collect(),
        next: v.next.map(|v| v.to_string()),
        omitted: v.omitted,
    }
}

impl KernelService {
    pub(super) async fn authorized_query_social_refs(
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
            .social_cognition
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
        let scope = SocialScope {
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
            .require_social()?
            .select(
                subject,
                SocialSelectionRequest {
                    scope: &scope,
                    source_read: &read,
                    max_items: 2048,
                    max_bytes: 4 * 1024 * 1024,
                    after: None,
                    expression: None,
                    endpoint: None,
                    counterpart: None,
                    at: time(modifiers.and_then(|m| m.clock_now))?,
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
