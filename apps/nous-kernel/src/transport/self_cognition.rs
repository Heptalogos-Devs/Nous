// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_core::*;
use nous_self::{
    AdoptSelf, NarrativeContent, ReviseSelf, SelfContent, SelfException, SelfFacetContent,
    SelfRecord, SelfScope,
};

fn content(input: p::SelfContent) -> Result<SelfContent> {
    Ok(match required(input.content, "Self content")? {
        p::self_content::Content::Facet(v) => {
            let scope = required(v.scope, "Self scope")?;
            SelfContent::SelfFacet(SelfFacetContent {
                kind: v.kind,
                statement: v.statement,
                epistemic_class: enum_value(&v.epistemic_class)?,
                scope: SelfScope {
                    description: scope.description,
                    purposes: scope.purposes,
                    entities: scope
                        .entity_refs
                        .into_iter()
                        .map(EntityRef::new)
                        .collect::<Result<_>>()?,
                    work_context: scope
                        .work_context_id
                        .map(|s| id(&s).map(WorkContextId))
                        .transpose()?,
                    conversation: scope.conversation_ref,
                },
                stance: v.stance,
                exceptions: v
                    .exceptions
                    .into_iter()
                    .map(|e| {
                        Ok(SelfException {
                            statement: e.statement,
                            basis: e.basis.into_iter().map(basis).collect::<Result<_>>()?,
                        })
                    })
                    .collect::<Result<_>>()?,
                basis: v.basis.into_iter().map(basis).collect::<Result<_>>()?,
                valid_time: temporal(v.valid_time)?,
                producer: v.producer.map(from_producer).transpose()?,
            })
        }
        p::self_content::Content::Narrative(v) => SelfContent::NarrativeThread(NarrativeContent {
            theme: v.theme,
            interpretation: v.interpretation,
            epistemic_class: enum_value(&v.epistemic_class)?,
            source_refs: v
                .source_refs
                .into_iter()
                .map(from_ref)
                .collect::<Result<_>>()?,
            basis: v.basis.into_iter().map(basis).collect::<Result<_>>()?,
            valid_time: temporal(v.valid_time)?,
            producer: v.producer.map(from_producer).transpose()?,
        }),
    })
}
fn content_proto(value: SelfContent) -> p::SelfContent {
    let content = match value {
        SelfContent::SelfFacet(v) => p::self_content::Content::Facet(p::SelfFacetContent {
            kind: v.kind,
            statement: v.statement,
            epistemic_class: v.epistemic_class.as_str().into(),
            scope: Some(p::SelfScope {
                description: v.scope.description,
                purposes: v.scope.purposes,
                entity_refs: v
                    .scope
                    .entities
                    .into_iter()
                    .map(|e| e.as_str().to_owned())
                    .collect(),
                work_context_id: v.scope.work_context.map(|w| w.0.to_string()),
                conversation_ref: v.scope.conversation,
            }),
            stance: v.stance,
            exceptions: v
                .exceptions
                .into_iter()
                .map(|e| p::SelfException {
                    statement: e.statement,
                    basis: e.basis.into_iter().map(basis_proto).collect(),
                })
                .collect(),
            basis: v.basis.into_iter().map(basis_proto).collect(),
            valid_time: Some(temporal_proto(&v.valid_time)),
            producer: v.producer.map(super::evidence::producer),
        }),
        SelfContent::NarrativeThread(v) => {
            p::self_content::Content::Narrative(p::NarrativeContent {
                theme: v.theme,
                interpretation: v.interpretation,
                epistemic_class: v.epistemic_class.as_str().into(),
                source_refs: v.source_refs.into_iter().map(to_ref).collect(),
                basis: v.basis.into_iter().map(basis_proto).collect(),
                valid_time: Some(temporal_proto(&v.valid_time)),
                producer: v.producer.map(super::evidence::producer),
            })
        }
    };
    p::SelfContent {
        content: Some(content),
    }
}
fn record(v: SelfRecord) -> p::SelfRecord {
    p::SelfRecord {
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
        formed_at: Some(timestamp(v.formed_at)),
        recorded_at: Some(timestamp(v.recorded_at)),
        evidence: v
            .evidence
            .into_iter()
            .map(|e| p::SelfEvidence {
                role: e.role,
                basis: Some(basis_proto(e.basis)),
                recorded_at: Some(timestamp(e.recorded_at)),
            })
            .collect(),
    }
}
fn self_ref(input: Option<p::CognitiveRef>, exact: bool) -> Result<CognitiveRef> {
    let r = from_ref(required(input, "Self reference")?)?;
    if r.domain() != CognitiveDomain::SelfCognition || r.is_cognition_revision() != exact {
        return Err(Error::Invalid(
            "Self reference has wrong family or precision".into(),
        ));
    }
    Ok(r)
}
#[tonic::async_trait]
impl p::self_service_server::SelfService for KernelService {
    async fn adopt(
        &self,
        r: Request<p::AdoptSelfRequest>,
    ) -> std::result::Result<Response<p::SelfRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_self()?
                    .adopt(AdoptSelf {
                        subject: SubjectId(id(&r.subject_id)?),
                        operation_id: OperationId(id(&r.operation_id)?),
                        content: content(required(r.content, "Self content")?)?,
                        reason: r.reason,
                    })
                    .await?,
            ))
        })
        .await
    }
    async fn revise(
        &self,
        r: Request<p::ReviseSelfRequest>,
    ) -> std::result::Result<Response<p::SelfRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let reference = self_ref(r.target_revision, true)?;
            let current = self
                .0
                .require_self()?
                .get(subject, reference.uuid().expect("Self UUID"), true)
                .await?;
            if current.reference() != reference {
                return Err(Error::Invalid("Self reference family mismatch".into()));
            }
            Ok(record(
                self.0
                    .require_self()?
                    .revise(ReviseSelf {
                        subject,
                        operation_id: OperationId(id(&r.operation_id)?),
                        object_id: current.object_id,
                        expected_revision: current.revision_id,
                        expected_epoch: r.expected_object_epoch,
                        content: content(required(r.content, "Self content")?)?,
                        reason: r.reason,
                    })
                    .await?,
            ))
        })
        .await
    }
    async fn add_evidence(
        &self,
        r: Request<p::AddSelfEvidenceRequest>,
    ) -> std::result::Result<Response<p::SelfRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_self()?
                    .add_evidence(
                        SubjectId(id(&r.subject_id)?),
                        OperationId(id(&r.operation_id)?),
                        self_ref(r.target_revision, true)?,
                        r.expected_object_epoch,
                        &r.role,
                        basis(required(r.basis, "Self basis")?)?,
                    )
                    .await?,
            ))
        })
        .await
    }
    async fn get(
        &self,
        r: Request<p::ObjectRequest>,
    ) -> std::result::Result<Response<p::SelfRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_self()?
                    .get(SubjectId(id(&r.subject_id)?), id(&r.id)?, false)
                    .await?,
            ))
        })
        .await
    }
    async fn get_revision(
        &self,
        r: Request<p::ObjectRequest>,
    ) -> std::result::Result<Response<p::SelfRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            Ok(record(
                self.0
                    .require_self()?
                    .get(SubjectId(id(&r.subject_id)?), id(&r.id)?, true)
                    .await?,
            ))
        })
        .await
    }
    async fn list(
        &self,
        r: Request<p::SelfListRequest>,
    ) -> std::result::Result<Response<p::SelfListResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let scope = format!("self:{}:{}", subject.0, r.family);
            let (limit, last) = page(r.page, &scope)?;
            let (items, more) = self
                .0
                .require_self()?
                .list(subject, &r.family, last, limit as usize)
                .await?;
            let next_page_token = if more {
                next_token(&scope, items.last().expect("nonempty page").object_id)
            } else {
                String::new()
            };
            Ok(p::SelfListResponse {
                items: items.into_iter().map(record).collect(),
                next_page_token,
            })
        })
        .await
    }
    async fn list_revisions(
        &self,
        r: Request<p::SelfHistoryRequest>,
    ) -> std::result::Result<Response<p::SelfListResponse>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let subject = SubjectId(id(&r.subject_id)?);
            let reference = self_ref(r.object_ref, false)?;
            let object = reference.uuid().expect("Self UUID");
            if self
                .0
                .require_self()?
                .get(subject, object, false)
                .await?
                .object_reference()
                != reference
            {
                return Err(Error::Invalid("Self reference family mismatch".into()));
            }
            let scope = format!("self-history:{}:{}", subject.0, object);
            let (limit, last) = page(r.page, &scope)?;
            let (items, more) = self
                .0
                .require_self()?
                .history(subject, object, last, limit as usize)
                .await?;
            let next_page_token = if more {
                next_token(&scope, items.last().expect("nonempty page").revision_id)
            } else {
                String::new()
            };
            Ok(p::SelfListResponse {
                items: items.into_iter().map(record).collect(),
                next_page_token,
            })
        })
        .await
    }
    async fn set_lifecycle(
        &self,
        r: Request<p::SelfLifecycleRequest>,
    ) -> std::result::Result<Response<p::SelfRecord>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            let m = required(r.mutation, "Self mutation")?;
            Ok(record(
                self.0
                    .require_self()?
                    .lifecycle(
                        SubjectId(id(&m.subject_id)?),
                        OperationId(id(&m.operation_id)?),
                        self_ref(m.object_ref, false)?,
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
        r: Request<p::SelfMutationRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let r = r.into_inner();
        rpc_reply(async {
            self.0
                .require_self()?
                .purge(
                    SubjectId(id(&r.subject_id)?),
                    OperationId(id(&r.operation_id)?),
                    self_ref(r.object_ref, false)?,
                    r.expected_object_epoch,
                )
                .await
        })
        .await
    }
}
