use super::*;
use crate::import_models as model;
use futures::{FutureExt, future::BoxFuture};
use nous_foundation::models::InvocationMetadata;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Default)]
pub(super) struct Semantic {
    pub call_id: Option<ModelCallId>,
    pub events: Vec<String>,
    pub entities: Vec<String>,
    pub relations: Vec<model::ImportTriple>,
}
pub(super) fn semantic<'a>(
    app: &'a App,
    job: &'a JobRecord,
    unit: &'a ImportTextUnit,
    input: model::ImportSemanticInput,
    enabled: bool,
    cancel: &'a CancellationToken,
) -> BoxFuture<'a, Result<Option<Semantic>>> {
    if !enabled || unit.effective_strategy == ImportTextStrategy::Quote {
        return async { Ok(Some(Semantic::default())) }.boxed();
    }
    match unit.effective_strategy {
        ImportTextStrategy::Factual => factual(app, job, input, cancel).boxed(),
        ImportTextStrategy::Narrative | ImportTextStrategy::ChatLog => {
            narrative(app, job, input, cancel).boxed()
        }
        _ => async { Err(Error::invalid("unit has no executable semantic strategy")) }.boxed(),
    }
}
async fn narrative(
    app: &App,
    job: &JobRecord,
    input: model::ImportSemanticInput,
    cancel: &CancellationToken,
) -> Result<Option<Semantic>> {
    let (ns, subject) = job.subject_scope()?;
    let plan = app.model.plan(
        ConfigScope::Subject {
            namespace_id: ns.clone(),
            subject_id: subject.clone(),
        },
        model::NARRATIVE,
        cancel.clone(),
    );
    let call=app.calls.invoke::<InvocationMetadata<model::ImportSemanticInput>,model::NarrativeExtraction,_,_,_,_>(
        &job.id,"semantic",cancel,|attempt|model::prepare_semantic(&plan,attempt,input.clone()),|prepared|model::narrative(&app.model,prepared),None).await?;
    let Some(call) = call else {
        return Ok(None);
    };
    let output: model::NarrativeExtraction = output(&call)?;
    Ok(Some(Semantic {
        call_id: Some(call.id),
        events: output.events,
        entities: vec![],
        relations: output.relations,
    }))
}
async fn factual(
    app: &App,
    job: &JobRecord,
    input: model::ImportSemanticInput,
    cancel: &CancellationToken,
) -> Result<Option<Semantic>> {
    let (ns, subject) = job.subject_scope()?;
    let plan = app.model.plan(
        ConfigScope::Subject {
            namespace_id: ns.clone(),
            subject_id: subject.clone(),
        },
        model::FACTUAL,
        cancel.clone(),
    );
    let call=app.calls.invoke::<InvocationMetadata<model::ImportSemanticInput>,model::FactualExtraction,_,_,_,_>(
        &job.id,"semantic",cancel,|attempt|model::prepare_semantic(&plan,attempt,input.clone()),|prepared|model::factual(&app.model,prepared),None).await?;
    let Some(call) = call else {
        return Ok(None);
    };
    let output: model::FactualExtraction = output(&call)?;
    Ok(Some(Semantic {
        call_id: Some(call.id),
        events: vec![],
        entities: output.entities,
        relations: output.triples,
    }))
}
pub(super) fn output<T: DeserializeOwned>(call: &ModelCall) -> Result<T> {
    let value = call
        .response
        .as_ref()
        .and_then(|value| value.get("result"))
        .ok_or_else(|| Error::internal("successful import call has no typed result"))?;
    Ok(serde_json::from_value(value.clone())?)
}

pub(super) async fn input(
    app: &App,
    job: &JobRecord,
    plan: &UnitPlan,
    cancel: &CancellationToken,
) -> Result<(
    ImportTextUnit,
    model::ImportSemanticInput,
    Option<TimePoint>,
)> {
    let (ns, subject) = job.subject_scope()?;
    let ctx = job.call_context(&plan.caller_id)?;
    let inputs = app
        .retry_import_inputs(
            &ctx,
            RetryImportInputsRequest {
                namespace_id: ns.clone(),
                subject_id: subject.clone(),
                catalog_id: plan.catalog_id.clone(),
                unit_ids: vec![plan.unit_id.clone()],
            },
            cancel,
        )
        .await?;
    let unit = inputs
        .units
        .into_iter()
        .next()
        .ok_or_else(|| Error::new(ErrorCode::NotFound, "processing unit is not retained"))?;
    let source = app
        .sources
        .resolve(
            &ctx,
            ns,
            subject,
            &nous_domain::media::MediaReference::SourceRegion {
                id: unit.source_region_id.clone(),
            },
        )
        .await?;
    let text = app.sources.text(&source, cancel).await?;
    if text != unit.text {
        return Err(Error::new(
            ErrorCode::Conflict,
            "unit differs from its original source region",
        ));
    }
    let mut context = unit.context.clone();
    let mut actual = Vec::<Value>::new();
    for id in &unit.context_region_ids {
        let reference = nous_domain::media::MediaReference::SourceRegion { id: id.clone() };
        let snapshot = app.sources.resolve(&ctx, ns, subject, &reference).await?;
        actual.push(json!({"source":reference,"text":app.sources.text(&snapshot,cancel).await?}));
    }
    context["source_context"] = json!(actual);
    let reference = inputs
        .catalog
        .documents
        .iter()
        .find(|document| document.input_source_id == unit.input_source_id)
        .ok_or_else(|| Error::internal("unit has no frozen document options"))?
        .effective_options
        .reference_time
        .clone();
    let input = model::ImportSemanticInput {
        namespace_id: ns.clone(),
        subject_id: subject.clone(),
        unit_id: unit.id.to_string(),
        source_refs: unit.source_requirements(),
        text,
        context,
    };
    Ok((unit, input, reference))
}
