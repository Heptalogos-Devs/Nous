use super::*;
use minijinja::{
    Environment, UndefinedBehavior,
    value::{Serde, Value as TemplateValue},
};
use nous_domain::{Error, ErrorCode, Result, validate_prompt_body};
use std::{collections::BTreeMap, io::Write};

pub fn validate_template(id: &str, body: &str) -> Result<Vec<String>> {
    validate_prompt_body(body)?;
    let environment = environment();
    let template = environment
        .template_from_named_str(id, body)
        .map_err(|error| Error::invalid(format!("prompt template is invalid: {error}")))?;
    let mut variables = template
        .undeclared_variables(false)
        .into_iter()
        .collect::<Vec<_>>();
    variables.sort();
    Ok(variables)
}
pub(super) fn render(
    asset: &PromptTemplate,
    arguments: &PromptArguments,
    override_text: Option<&str>,
) -> Result<PromptSnapshot> {
    let source = override_text.unwrap_or(&asset.text);
    validate_prompt_body(source)?;
    if serde_json::to_vec(arguments)?.len() > 1024 * 1024 {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "prompt arguments exceed 1 MiB",
        ));
    }
    let environment = environment();
    let template = environment
        .template_from_named_str(&asset.id, source)
        .map_err(|error| Error::invalid(format!("prompt template is invalid: {error}")))?;
    let context = arguments
        .iter()
        .map(|(name, arg)| (name, &arg.value))
        .collect::<BTreeMap<_, _>>();
    let mut output = BoundedOutput(Vec::new());
    template
        .render_captured_to(TemplateValue::from(Serde(context)), &mut output)
        .map_err(|error| Error::invalid(format!("prompt rendering failed: {error}")))?;
    let text = String::from_utf8(output.0)
        .map_err(|_| Error::internal("prompt renderer returned invalid UTF-8"))?;
    if text.trim().is_empty() {
        return Err(Error::invalid("rendered prompt must be nonempty"));
    }
    Ok(PromptSnapshot {
        resource: asset.clone(),
        template: source.into(),
        template_sha256: digest(source),
        arguments: arguments.clone(),
        sha256: digest(&text),
        text,
        overridden: override_text.is_some(),
    })
}
fn environment() -> Environment<'static> {
    let mut environment = Environment::new();
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    environment.set_fuel(Some(50000));
    environment.set_recursion_limit(32);
    environment
}
struct BoundedOutput(Vec<u8>);
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_RENDERED_BYTES {
            return Err(std::io::Error::other("rendered prompt exceeds 256 KiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
