use nous_domain::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportSemanticInput {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub unit_id: String,
    pub source_refs: Vec<SourceReadRef>,
    pub text: String,
    pub context: Value,
}
impl ImportSemanticInput {
    pub fn validate(&self) -> Result<()> {
        validate(
            &self.namespace_id,
            &self.subject_id,
            &self.unit_id,
            &self.source_refs,
            &self.text,
            &self.context,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportTimeInput {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub unit_id: String,
    pub source_refs: Vec<SourceReadRef>,
    pub text: String,
    pub context: Value,
    pub reference_time: Option<TimePoint>,
}
impl ImportTimeInput {
    pub fn validate(&self) -> Result<()> {
        validate(
            &self.namespace_id,
            &self.subject_id,
            &self.unit_id,
            &self.source_refs,
            &self.text,
            &self.context,
        )?;
        if let Some(reference) = &self.reference_time {
            reference.validate()?;
        }
        Ok(())
    }
}

fn validate(
    namespace: &NamespaceId,
    subject: &SubjectId,
    unit: &str,
    sources: &[SourceReadRef],
    text: &str,
    context: &Value,
) -> Result<()> {
    namespace.validate()?;
    subject.validate()?;
    validate_name(unit, "import unit")?;
    if sources.is_empty() || sources.len() > 128 || text.trim().is_empty() || !context.is_object() {
        return Err(Error::invalid(
            "import model input requires 1..128 exact sources, nonempty text and object context",
        ));
    }
    let mut unique = std::collections::BTreeSet::new();
    for source in sources {
        source.validate()?;
        if !unique.insert(source) {
            return Err(Error::invalid("import model input repeats a source"));
        }
    }
    Ok(())
}
