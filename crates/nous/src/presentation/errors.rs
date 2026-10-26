use nous_domain::Error;
use nous_foundation::localization::Presentation;
use serde_json::{Value, json};

pub fn error_document(error: &Error, presentation: &Presentation) -> Value {
    json!({"error": error, "summary": summary(error,presentation), "language": presentation.locale()})
}
pub fn error_text(error: &Error, presentation: &Presentation) -> String {
    let mut text = format!(
        "{}\n{}: {}",
        summary(error, presentation),
        super::label("details", presentation),
        error.message
    );
    if let Some(trace) = &error.trace_id {
        text.push_str(&format!(
            "\n{}: {trace}",
            super::label("trace_id", presentation)
        ));
    }
    if !error.context.is_empty() {
        text.push_str(&format!(
            "\n{}",
            super::render(&json!(error.context), presentation)
        ));
    }
    let mut cause = error.cause.as_ref();
    while let Some(value) = cause {
        if value.message != error.message {
            text.push_str(&format!(
                "\n{}: {}",
                super::label("cause", presentation),
                value.message
            ));
        }
        cause = value.source.as_deref();
    }
    text
}
fn summary(error: &Error, presentation: &Presentation) -> String {
    presentation
        .error(error)
        .unwrap_or_else(|_| format!("{:?}", error.code))
}
