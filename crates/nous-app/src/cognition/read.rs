use crate::App;
use nous_domain::*;
use nous_storage::SourceClosure;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetrievalSourceRequest {
    pub subject_id: SubjectId,
    pub reference: RetrievalTarget,
    pub observation_id: Option<ObservationId>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CognitiveSourceSnapshot {
    pub object: RetrievalRecord,
    pub closure: SourceClosure,
    pub watermark: i64,
}
impl App {
    pub async fn retrieval_source(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        target: &RetrievalTarget,
    ) -> Result<Value> {
        self.read_retrieval_source(
            ctx,
            RetrievalSourceRequest {
                subject_id: subject.clone(),
                reference: target.clone(),
                observation_id: None,
            },
        )
        .await
    }
    pub async fn read_retrieval_source(
        &self,
        ctx: &CallContext,
        request: RetrievalSourceRequest,
    ) -> Result<Value> {
        request.reference.validate()?;
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        let context = ctx.narrow(AccessScope::Subject {
            namespace_id: namespace.clone(),
            subject_id: request.subject_id.clone(),
        })?;
        match request.reference {
            RetrievalTarget::Cognition(reference) => {
                if request.observation_id.is_some() {
                    return Err(Error::invalid(
                        "a cognition carries its own actual source event",
                    ));
                }
                if reference.kind == CognitiveKind::Memory {
                    Ok(json!(
                        self.memory_source(
                            &context,
                            &MemoryId(reference.id),
                            Some(reference.revision)
                        )
                        .await?
                    ))
                } else {
                    let object = self
                        .get_structured(
                            &context,
                            &CognitiveId(reference.id.clone()),
                            Some(reference.revision),
                        )
                        .await?;
                    if object.kind() != reference.kind {
                        return Err(Error::invalid(
                            "source reference kind differs from its stored object",
                        ));
                    }
                    self.structured_source(
                        &context,
                        &CognitiveId(reference.id),
                        Some(reference.revision),
                    )
                    .await
                }
            }
            RetrievalTarget::Media(reference) => Ok(json!(match request.observation_id {
                Some(observation) =>
                    self.sources
                        .lineage_for_event(
                            &context,
                            &namespace,
                            &request.subject_id,
                            &reference,
                            &observation
                        )
                        .await?,
                None =>
                    self.sources
                        .lineage(&context, &namespace, &request.subject_id, &reference)
                        .await?,
            })),
        }
    }
    pub async fn read_cognitive_source(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &CognitiveRef,
    ) -> Result<CognitiveSourceSnapshot> {
        ctx.require_subject(namespace, subject)?;
        reference.validate()?;
        let mut view = self.database.read_view().await?;
        view.subject(namespace, subject).await?;
        view.require_source_read(
            ctx,
            namespace,
            subject,
            &RetrievalTarget::Cognition(reference.clone()),
        )
        .await?;
        let filters = QueryFilters::default();
        let options = nous_storage::QuerySnapshotOptions {
            space_id: "",
            as_of_commit: None,
            limit: 1,
            source_read: None,
            filters: &filters,
            history: false,
        };
        let snapshot = view
            .cognitive_reference_snapshot(namespace, subject, &options, reference)
            .await?;
        let selected = match (
            snapshot.memories.into_iter().next(),
            snapshot.structured.into_iter().next(),
        ) {
            (Some(entry), None) => Some((RetrievalRecord::Memory(entry.memory), entry.source)),
            (None, Some(entry)) => Some((RetrievalRecord::Structured(entry.object), entry.source)),
            _ => None,
        };
        let (object, closure) = selected
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "cognitive source not found"))?;
        if object.deleted() || !closure.complete {
            return Err(Error::new(
                ErrorCode::NotFound,
                "cognitive source is no longer available",
            ));
        }
        for root in &closure.roots {
            if !view
                .subject_has_material_access(namespace, subject, &root.material_id)
                .await?
            {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "cognitive source is outside the subject's admitted material",
                ));
            }
        }
        let watermark = snapshot.watermark;
        view.finish().await?;
        Ok(CognitiveSourceSnapshot {
            object,
            closure,
            watermark,
        })
    }
}
