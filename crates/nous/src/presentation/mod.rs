mod errors;
mod language;
mod query;
pub use errors::{error_document, error_text};
pub use language::{cli_language, current, in_language, set_cli};
use nous_foundation::localization::{FluentArgs, Presentation};
pub use query::query_result;
use serde_json::Value;

pub fn render(value: &Value, presentation: &Presentation) -> String {
    if value.get("hits").is_some()
        && value.get("coverage").is_some()
        && let Ok(result) = serde_json::from_value::<nous_domain::QueryResult>(value.clone())
    {
        return query_result(&result, presentation);
    }
    let mut output = String::new();
    if let Some(result) = value
        .get("result")
        .filter(|_| value.get("operation_id").is_some())
    {
        format_value(result, 0, &mut output, presentation);
        for key in ["operation_id", "commit_seq", "job_ids"] {
            if let Some(value) = value.get(key) {
                field(key, value, 0, &mut output, presentation);
            }
        }
        if let Some(trace) = value.pointer("/context/trace_id") {
            field("trace_id", trace, 0, &mut output, presentation);
        }
    } else {
        format_value(value, 0, &mut output, presentation);
    }
    output.trim_end().to_owned()
}

fn field(key: &str, value: &Value, depth: usize, output: &mut String, presentation: &Presentation) {
    output.push_str(&"  ".repeat(depth));
    output.push_str(&label(key, presentation));
    output.push(':');
    if is_scalar(value) {
        output.push(' ');
        format_value(value, 0, output, presentation);
    } else {
        output.push('\n');
        format_value(value, depth + 1, output, presentation);
    }
}
fn is_scalar(value: &Value) -> bool {
    match value {
        Value::String(text) if text.contains('\n') => false,
        Value::Object(_) => false,
        Value::Array(items) => items
            .iter()
            .all(|item| !item.is_array() && !item.is_object()),
        _ => true,
    }
}
fn format_value(value: &Value, depth: usize, output: &mut String, presentation: &Presentation) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                field(key, value, depth, output, presentation);
            }
        }
        Value::Array(items) if items.is_empty() => output.push_str("[]\n"),
        Value::Array(items) if items.len() > 16 && items.iter().all(Value::is_number) => {
            let mut args = FluentArgs::new();
            args.set("count", items.len());
            output.push_str(
                &presentation
                    .message("cli-numbers", Some(&args))
                    .unwrap_or_else(|_| format!("[{}]", items.len())),
            );
            output.push('\n');
        }
        Value::Array(items) if is_scalar(value) => {
            output.push_str(&items.iter().map(scalar).collect::<Vec<_>>().join(", "));
            output.push('\n');
        }
        Value::Array(items) => {
            for item in items {
                output.push_str(&"  ".repeat(depth));
                output.push_str("-\n");
                format_value(item, depth + 1, output, presentation);
            }
        }
        _ => {
            let text = scalar(value);
            for line in text.lines() {
                output.push_str(&"  ".repeat(depth));
                output.push_str(line);
                output.push('\n');
            }
            if text.is_empty() {
                output.push('\n');
            }
        }
    }
}
pub fn label(key: &str, presentation: &Presentation) -> String {
    let key_id = match key {
        "operation_id" => "operation",
        "commit_seq" => "commit-seq",
        "source_roots" => "source-roots",
        key => key,
    };
    presentation
        .message(&format!("cli-{}", key_id.replace('_', "-")), None)
        .unwrap_or_else(|_| key.to_owned())
}
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text)
            if !text
                .chars()
                .any(|ch| ch.is_control() && ch != '\n' && ch != '\t') =>
        {
            text.clone()
        }
        _ => value.to_string(),
    }
}
