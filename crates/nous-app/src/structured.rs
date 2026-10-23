//! Typed cognition operations share the existing authority transaction and source rules.
use super::*;
use nous_retrieval::{IndexDocument, RetrievalIndex};
use serde::Deserialize;
use std::collections::BTreeMap;
mod formation;
mod lineage;
pub use formation::{FormStructuredRequest, FormationTarget};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredQuery {
    pub subject_id: SubjectId,
    pub kind: Option<CognitiveKind>,
    #[serde(default)]
    pub text: String,
    pub as_of_commit: Option<i64>,
    #[serde(default)]
    pub include_deleted: bool,
    pub time: Option<TimeFilter>,
    #[serde(default = "query_limit")]
    pub limit: usize,
}
fn query_limit() -> usize {
    10
}

impl App {
    pub async fn create_structured(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        input: CreateStructuredRequest,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&input.namespace_id, &input.subject_id)?;
        if input.expected_revision != 0 {
            return Err(Error::invalid("creation requires expected_revision=0"));
        }
        if input.producer_call_id.is_some() {
            return Err(Error::invalid(
                "model provenance is assigned by the formation worker",
            ));
        }
        let scope = format!(
            "namespace:{}/subject:{}",
            input.namespace_id, input.subject_id
        );
        let key = operation_key(
            ctx,
            "cognition.create",
            &scope,
            operation_id,
            &serde_json::to_value(&input)?,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let subject = tx.subject(&input.namespace_id, &input.subject_id).await?;
        if !subject.capabilities.contains(&Capability::Memory) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "subject has no memory capability",
            ));
        }
        let mut times = input.times;
        times.recorded = cognition::utc_point();
        let object = StructuredObject {
            id: CognitiveId::new(),
            namespace_id: input.namespace_id,
            subject_id: input.subject_id,
            revision: 1,
            body: input.body,
            evidence: input.evidence,
            times,
            producer_call_id: None,
            recorded_at: Utc::now().to_rfc3339(),
            commit_seq: tx.commit_seq(),
            deleted: false,
        };
        tx.save_structured(&object, None).await?;
        tx.commit(serde_json::to_value(&object)?).await
    }

    pub async fn revise_structured(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        input: ReviseStructuredRequest,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.structured_namespace(&input.id).await?;
        let previous = self
            .database
            .structured_object(&namespace, &input.id)
            .await?;
        ctx.require_subject(&namespace, &previous.subject_id)?;
        if input.producer_call_id.is_some() {
            return Err(Error::invalid(
                "model provenance is assigned by the formation worker",
            ));
        }
        let scope = format!("namespace:{namespace}/subject:{}", previous.subject_id);
        let key = operation_key(
            ctx,
            "cognition.revise",
            &scope,
            operation_id,
            &serde_json::to_value(&input)?,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut current = tx.structured_object(&namespace, &input.id).await?;
        if current.deleted {
            return Err(Error::new(
                ErrorCode::Conflict,
                "restore the cognitive object before revising it",
            ));
        }
        current.revision = input
            .expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::invalid("revision overflow"))?;
        current.body = input.body;
        current.evidence = input.evidence;
        current.times = input.times;
        current.times.recorded = cognition::utc_point();
        current.recorded_at = Utc::now().to_rfc3339();
        current.commit_seq = tx.commit_seq();
        current.producer_call_id = None;
        tx.save_structured(&current, Some(input.expected_revision))
            .await?;
        tx.commit(serde_json::to_value(current)?).await
    }

    pub async fn get_structured(
        &self,
        ctx: &CallContext,
        id: &CognitiveId,
        revision: Option<i64>,
    ) -> Result<StructuredObject> {
        let namespace = self.database.structured_namespace(id).await?;
        let object = if let Some(revision) = revision {
            self.database
                .structured_revision(&namespace, id, revision)
                .await?
        } else {
            self.database.structured_object(&namespace, id).await?
        };
        ctx.require_subject(&namespace, &object.subject_id)?;
        if revision.is_none() && object.deleted {
            return Err(Error::new(
                ErrorCode::NotFound,
                "cognitive object is deleted",
            ));
        }
        Ok(object)
    }

    pub async fn structured_history(
        &self,
        ctx: &CallContext,
        id: &CognitiveId,
    ) -> Result<Vec<StructuredObject>> {
        let namespace = self.database.structured_namespace(id).await?;
        let object = self.database.structured_object(&namespace, id).await?;
        ctx.require_subject(&namespace, &object.subject_id)?;
        self.database.structured_history(&namespace, id).await
    }

    pub async fn set_structured_deleted(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &CognitiveId,
        expected_revision: i64,
        deleted: bool,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.structured_namespace(id).await?;
        let object = self.database.structured_object(&namespace, id).await?;
        ctx.require_subject(&namespace, &object.subject_id)?;
        let scope = format!("namespace:{namespace}/subject:{}", object.subject_id);
        let key = operation_key(
            ctx,
            "cognition.lifecycle",
            &scope,
            operation_id,
            &json!({"id":id,"expected_revision":expected_revision,"deleted":deleted}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut object = tx.structured_object(&namespace, id).await?;
        object.revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::invalid("revision overflow"))?;
        object.deleted = deleted;
        object.commit_seq = tx.commit_seq();
        object.recorded_at = Utc::now().to_rfc3339();
        object.times.recorded = cognition::utc_point();
        object.producer_call_id = None;
        tx.save_structured(&object, Some(expected_revision)).await?;
        tx.commit(serde_json::to_value(object)?).await
    }

    pub async fn structured_source(
        &self,
        ctx: &CallContext,
        id: &CognitiveId,
        revision: Option<i64>,
    ) -> Result<Value> {
        let object = self.get_structured(ctx, id, revision).await?;
        let closure = self
            .database
            .source_closure(&object.namespace_id, id, Some(object.revision))
            .await?;
        let mut materials = Vec::new();
        for root in &closure.roots {
            match self.get_material(ctx, &root.material_id).await {
                Ok(material) => materials.push(material),
                Err(error) if error.code == ErrorCode::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let producer = match &object.producer_call_id {
            Some(id) => Some(self.read_model_call(ctx, id).await?),
            None => None,
        };
        Ok(json!({"object":object,"closure":closure,"materials":materials,"producer":producer}))
    }

    pub async fn concept_by_alias(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        alias: &str,
    ) -> Result<StructuredObject> {
        let ns = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&ns, subject)?;
        self.database.concept_by_alias(&ns, subject, alias).await
    }

    pub async fn query_structured(
        &self,
        ctx: &CallContext,
        request: StructuredQuery,
    ) -> Result<Value> {
        if !(1..=100).contains(&request.limit) || request.text.len() > 4096 {
            return Err(Error::invalid(
                "query limit must be 1..100 and text at most 4096 bytes",
            ));
        }
        let ns = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&ns, &request.subject_id)?;
        let snapshot = self
            .database
            .subject_structured_snapshot(
                &ns,
                &request.subject_id,
                request.kind,
                request.as_of_commit,
                5000,
            )
            .await?;
        if !snapshot.complete {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "structured query snapshot exceeds 5000 objects",
            ));
        }
        let objects: Vec<_> = snapshot
            .objects
            .into_iter()
            .filter(|object| {
                (request.include_deleted || !object.deleted)
                    && cognition::matches_times(&object.times, request.time.as_ref())
            })
            .collect();
        if let Some(time) = &request.time {
            TimePoint {
                domain: time.range.domain.clone(),
                coordinate: 0,
            }
            .validate()?;
            if matches!((time.range.start,time.range.end),(Some(start),Some(end)) if start>end) {
                return Err(Error::invalid("time range start exceeds end"));
            }
        }
        let documents: Vec<_> = objects
            .iter()
            .map(|object| IndexDocument {
                id: object.id.to_string(),
                namespace_id: ns.to_string(),
                subject_id: request.subject_id.to_string(),
                revision: object.revision,
                commit_seq: object.commit_seq,
                text: representation(&object.body),
                deleted: false,
            })
            .collect();
        let parent = self.paths.temp.clone();
        let text = request.text.clone();
        let sub = request.subject_id.clone();
        let scope = ns.clone();
        let upper = snapshot.watermark;
        let page = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            // A bounded authority snapshot gives historical and current queries the
            // same semantics. Scratch indexes are removed after each query.
            let directory = tempfile::Builder::new()
                .prefix("cognition-query-")
                .tempdir_in(parent)?;
            let index = RetrievalIndex::open(directory.path())?;
            index.rebuild(&documents, upper)?;
            let page = index.search(&text, scope.as_str(), sub.as_str(), 5000);
            index.close()?;
            page
        })
        .await
        .map_err(|e| Error::internal(e.to_string()))?
        .map_err(|e| Error::new(ErrorCode::DependencyUnavailable, e.to_string()))?;
        let mut by_id: BTreeMap<_, _> = objects
            .into_iter()
            .map(|object| (object.id.clone(), object))
            .collect();
        let ranked = if request.text.trim().is_empty() {
            by_id
                .values()
                .map(|object| (object.id.clone(), 0.0))
                .collect::<Vec<_>>()
        } else {
            page.hits
                .into_iter()
                .map(|hit| Ok((hit.id.parse()?, hit.score)))
                .collect::<Result<Vec<_>>>()?
        };
        let mut hits = Vec::new();
        for (id, score) in ranked {
            let Some(object) = by_id.remove(&id) else {
                continue;
            };
            if request.as_of_commit.is_none() {
                let current = self.database.structured_object(&ns, &id).await?;
                if current.revision != object.revision
                    || (!request.include_deleted && current.deleted)
                {
                    continue;
                }
            }
            hits.push(json!({"reference":object.reference(),"object":object,"score":score}));
            if hits.len() >= request.limit {
                break;
            }
        }
        Ok(
            json!({"hits":hits,"authority_commit_seq":upper,"complete":true,"generation":page.coverage.generation}),
        )
    }
}

pub(crate) fn representation(body: &CognitiveContent) -> String {
    match body {
        CognitiveContent::MemorySemantics(value) => value.qualifications.join("\n"),
        CognitiveContent::Concept(value) => format!(
            "{}\n{}\n{}",
            value.label,
            value.description,
            value.aliases.join(" ")
        ),
        CognitiveContent::Episode(value) => format!(
            "{}\n{}\n{}",
            value.title, value.track, value.boundary_explanation
        ),
        CognitiveContent::Journal(value) => format!(
            "{}\n{}\n{}",
            value.title,
            value.narrative,
            value
                .points
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        ),
        CognitiveContent::Schema(value) => format!(
            "{}\n{}\n{}\n{}",
            value.title, value.claim, value.scope.description, value.boundary
        ),
        CognitiveContent::Association(value) => {
            format!("{}\n{}", value.relation, value.description)
        }
    }
}
