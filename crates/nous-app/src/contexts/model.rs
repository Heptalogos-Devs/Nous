use nous_domain::{context_work::*, *};
use nous_foundation::{configuration::ConfigKey, models::*, prompts::PromptDefinition};
use serde::{Deserialize, Serialize};
use serde_json::json;
pub const ROLE: &str = "projection_steward";
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StewardSource {
    pub key: String,
    pub text: String,
    pub authority: String,
    pub sources: Vec<SourceReadRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StewardInput {
    pub preparation_id: ContextPreparationId,
    pub intent: String,
    pub context: Vec<StewardSource>,
    pub sources: Vec<StewardSource>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StewardProposal {
    pub selected_ids: Vec<String>,
    pub summary: Option<String>,
}
impl StewardProposal {
    pub fn validate(&self, input: &StewardInput) -> Result<()> {
        if self.selected_ids.len() > 64
            || self.summary.as_ref().is_some_and(|text| text.len() > 8192)
        {
            return Err(Error::invalid(
                "Steward output exceeds its selection or summary bound",
            ));
        }
        let mut selected = std::collections::BTreeSet::new();
        for key in &self.selected_ids {
            if !input.sources.iter().any(|source| &source.key == key) || !selected.insert(key) {
                return Err(Error::invalid(
                    "Steward selected an unknown or repeated catalog key",
                ));
            }
        }
        if self
            .summary
            .as_ref()
            .is_some_and(|summary| !summary.trim().is_empty())
            && selected.is_empty()
        {
            return Err(Error::invalid("Steward summary requires selected sources"));
        }
        Ok(())
    }
}
pub fn definition() -> RoleDefinition {
    RoleDefinition {
        id: ROLE,
        binding_key: ConfigKey::<Option<RoleBinding>>::new("models.roles.projection_steward"),
        requirements: ModelRequirements::structured_text(),
        prompt: Some(PromptDefinition {
            id: ROLE,
            template: include_str!("../prompts/context-steward.md"),
        }),
        contract: RoleContract::new::<StewardInput>(draft),
    }
}
pub(super) fn input(preparation: &ContextPreparation) -> StewardInput {
    StewardInput {
        preparation_id: preparation.id.clone(),
        intent: preparation.request.usage.intent.clone(),
        context: preparation
            .catalog
            .iter()
            .filter(|segment| segment.required)
            .map(|segment| source(segment, preparation.work_context.as_ref()))
            .collect(),
        sources: preparation
            .catalog
            .iter()
            .filter(|segment| !segment.required)
            .map(|segment| source(segment, None))
            .collect(),
    }
}
fn source(segment: &ContextSegment, task: Option<&WorkContext>) -> StewardSource {
    StewardSource {
        key: segment.key.clone(),
        text: segment.text.clone(),
        authority: serde_json::to_value(segment.authority)
            .expect("authority serializes")
            .as_str()
            .expect("authority string")
            .into(),
        sources: if matches!(segment.authority, ContextAuthority::TaskDirective) {
            task.map(|task| {
                vec![SourceReadRef::WorkContext {
                    id: task.id.clone(),
                }]
            })
            .unwrap_or_default()
        } else {
            segment.sources.iter().map(SourceReadRef::target).collect()
        },
    }
}
fn draft(input: StewardInput) -> Result<InvocationDraft> {
    let requirements = input
        .sources
        .iter()
        .chain(&input.context)
        .flat_map(|source| source.sources.clone())
        .collect();
    if input.sources.is_empty() || input.sources.len() > 64 {
        return Err(Error::invalid(
            "Steward requires 1..64 already authorized optional units",
        ));
    }
    let keys = input
        .sources
        .iter()
        .map(|source| source.key.clone())
        .collect::<Vec<_>>();
    let schema = json!({"type":"object","additionalProperties":false,"properties":{"selected_ids":{"type":"array","items":{"type":"string","enum":keys},"maxItems":64,"uniqueItems":true},"summary":{"anyOf":[{"type":"string","maxLength":8192},{"type":"null"}]}},"required":["selected_ids","summary"]});
    let catalog = input
        .sources
        .iter()
        .map(|source| json!({"key":source.key,"text":source.text,"authority":source.authority}))
        .collect::<Vec<_>>();
    let context = input
        .context
        .iter()
        .map(|source| json!({"key":source.key,"text":source.text,"authority":source.authority}))
        .collect::<Vec<_>>();
    Ok(InvocationDraft::chat(
        Default::default(),
        vec![ModelMessage::user(
            json!({"intent":input.intent,"context":context,"sources":catalog}).to_string(),
        )],
        OutputContract::JsonSchema {
            name: "context_steward".into(),
            schema,
        },
    )
    .with_sources(requirements))
}
pub async fn prepare(
    plan: &InvocationPlan,
    attempt: AttemptContext,
    input: StewardInput,
) -> std::result::Result<PreparedCall<InvocationMetadata<StewardInput>>, ModelFailure> {
    plan.prepare(attempt, input, Default::default()).await
}
pub async fn execute(
    runtime: &ModelRuntime,
    prepared: PreparedCall<InvocationMetadata<StewardInput>>,
) -> ModelExecution<InvocationMetadata<StewardInput>, StewardProposal> {
    runtime.execute(prepared).await.map(|output, metadata| {
        let ModelOutput::Text(text) = output else {
            return Err(ModelFailure::domain(Error::invalid(
                "Steward requires structured text output",
            )));
        };
        let value: StewardProposal = serde_json::from_str(&text)
            .map_err(|error| ModelFailure::domain(Error::invalid(error.to_string())))?;
        value
            .validate(&metadata.input)
            .map_err(ModelFailure::domain)?;
        Ok(value)
    })
}
