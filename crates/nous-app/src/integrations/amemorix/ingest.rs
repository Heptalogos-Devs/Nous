use super::{policy::AmemorixPolicy, time, types::*};
use crate::App;
use nous_domain::*;
use serde_json::{Value, json};

pub(super) async fn execute(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &IngestInput,
    policy: &AmemorixPolicy,
) -> Result<Value> {
    if !policy.permits(
        input.respect_filter,
        &input.chat_id,
        &input.group_id,
        &input.user_id,
    ) {
        return Ok(
            json!({"success":true,"stored_ids":[],"skipped_ids":[input.external_id],"detail":"chat_filtered","filtered":true}),
        );
    }
    if !input.metadata.is_object() {
        return Err(Error::invalid("A-Memorix metadata must be a JSON object"));
    }
    for relation in &input.relations {
        if relation.subject.trim().is_empty()
            || relation.predicate.trim().is_empty()
            || relation.object.trim().is_empty()
            || !relation.metadata.is_object()
            || relation
                .confidence
                .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(Error::invalid(
                "relation requires subject/predicate/object, object metadata and finite 0..1 confidence",
            ));
        }
    }
    let start = input
        .timestamp
        .as_ref()
        .or(input.time_start.as_ref())
        .map(time::point)
        .transpose()?;
    let interval = time::filter(input.time_start.as_ref(), input.time_end.as_ref())?;
    let mut metadata = input.metadata.clone();
    let object = metadata.as_object_mut().expect("validated metadata");
    for (key, value) in [
        ("external_id", json!(input.external_id)),
        ("source_type", json!(input.source_type)),
        ("chat_id", json!(input.chat_id)),
        ("person_ids", json!(input.person_ids)),
        ("participants", json!(input.participants)),
        ("tags", json!(input.tags)),
        ("user_id", json!(input.user_id)),
        ("group_id", json!(input.group_id)),
        ("timestamp", json!(input.timestamp)),
        ("time_start", json!(input.time_start)),
        ("time_end", json!(input.time_end)),
    ] {
        object.insert(key.into(), value);
    }
    if input.source_type == "web_import" {
        object.insert("scope_type".into(), json!("global"));
    }
    let source = source(input);
    let origin = ObservationOrigin::from_refs(
        if input.chat_id.is_empty() {
            None
        } else {
            Some(ConversationRef::new(input.chat_id.clone())?)
        },
        Some(ExternalObjectRef::new(input.external_id.clone())?),
    )?;
    let record = ImportRecord {
        external_id: input.external_id.clone(),
        text: input.text.clone(),
        source: Some(source),
        source_type: Some(input.source_type.clone()),
        event_time: start,
        observed_at: None,
        origin: origin.clone(),
        tags: input.tags.clone(),
        metadata,
        structured: Some(
            json!({"entities":input.entities,"relations":input.relations,"time_window":interval.map(|value|value.range)}),
        ),
    };
    let receipt = app
        .submit_import(
            ctx,
            &ulid::Ulid::generate().to_string(),
            ImportRequest {
                namespace_id: binding.namespace_id.clone(),
                subject_id: binding.subject_id.clone(),
                inputs: vec![ImportDocument {
                    name: None,
                    format: ImportFormat::Json,
                    input: ImportDocumentInput::Inline {
                        content: serde_json::to_string(&vec![record])?,
                    },
                    source: None,
                }],
                form_memory: policy.form_memory,
                completion: ImportCompletion::Accepted,
                origin,
            },
        )
        .await?;
    let job_id = receipt
        .result
        .get("job_id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::internal("import submission has no job identity"))?;
    let jobs = app
        .wait_jobs(
            ctx,
            &[job_id.to_owned()],
            policy.ingest_wait_ms,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await?;
    let job = jobs
        .first()
        .ok_or_else(|| Error::internal("import wait has no task result"))?;
    if job.state != JobState::Succeeded {
        return Err(job
            .error
            .clone()
            .unwrap_or_else(|| {
                Error::new(
                    if job.state == JobState::Cancelled {
                        ErrorCode::Cancelled
                    } else {
                        ErrorCode::DependencyUnavailable
                    },
                    "source import has not completed",
                )
            })
            .with_context("job_id", job_id)
            .with_context("import_result", json!(job.result)));
    }
    let result: ImportResult = serde_json::from_value(
        job.result
            .clone()
            .ok_or_else(|| Error::internal("import task has no admission results"))?,
    )?;
    let item = result
        .items
        .first()
        .ok_or_else(|| Error::internal("source admission returned no item"))?;
    if let Some(error) = &item.error {
        return Err(error.clone());
    }
    let admission = item
        .admission
        .as_ref()
        .ok_or_else(|| Error::internal("source import has no admitted identities"))?;
    let memory_id = admission.memory_id.clone();
    let source: MemorySource = serde_json::from_value(
        app.retrieval_source(
            ctx,
            &binding.subject_id,
            &RetrievalTarget::Cognition(CognitiveRef::memory(&memory_id, 1)),
        )
        .await?,
    )?;
    let memory = source.memory;
    let graph = super::graph::adopt(app, ctx, binding, input, &memory).await?;
    let stored_ids = if result.replayed > 0 {
        Vec::<String>::new()
    } else {
        std::iter::once(memory.id.to_string())
            .chain(graph.iter().map(ToString::to_string))
            .collect()
    };
    let skipped = if result.replayed > 0 {
        vec![memory.id.to_string()]
    } else {
        vec![]
    };
    Ok(
        json!({"success":true,"stored_ids":stored_ids,"skipped_ids":skipped,"detail":if result.replayed>0 {"exists"}else{"stored"},"operation":receipt,"admission":admission,"import_job_id":job_id,"structured_targets":graph,"time_start":input.time_start,"time_end":input.time_end}),
    )
}
fn source(input: &IngestInput) -> String {
    if input.source_type == "chat_summary" && !input.chat_id.is_empty() {
        format!("chat_summary:{}", input.chat_id)
    } else if input.source_type == "person_fact" && !input.person_ids.is_empty() {
        format!("person_fact:{}", input.person_ids[0])
    } else if input.chat_id.is_empty() {
        input.source_type.clone()
    } else {
        format!("{}:{}", input.source_type, input.chat_id)
    }
}
