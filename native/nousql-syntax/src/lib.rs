//! Pure NousQL grammar. Binding, clocks and planning belong to Python.
mod parser;
pub use parser::parse;
pub const MANUAL: &str = include_str!("manual.md");
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub enum ErrorCode {
    InvalidArgument,
    Internal,
    ResourceExhausted,
}
#[derive(Debug, Clone)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    pub start_byte: usize,
    pub end_byte: usize,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            start_byte: 0,
            end_byte: 0,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
    pub fn with_cause(mut self, cause: &impl std::fmt::Display) -> Self {
        self.message = format!("{}: {}", self.message, cause);
        self
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Error {}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum QueryExpression {
    Atom { source: String },
    Any { children: Vec<QueryExpression> },
    All { children: Vec<QueryExpression> },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectorKind {
    Entity,
    Tag,
    Schema,
    Resource,
    Object,
    Ref,
}
impl SelectorKind {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "e" | "entity" => Self::Entity,
            "tag" => Self::Tag,
            "schema" => Self::Schema,
            "r" | "resource" => Self::Resource,
            "object" => Self::Object,
            "ref" => Self::Ref,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Tag => "tag",
            Self::Schema => "schema",
            Self::Resource => "resource",
            Self::Object => "object",
            Self::Ref => "ref",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QueryCue {
    Concept {
        text: String,
    },
    Selector {
        selector: SelectorKind,
        locators: Vec<String>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Directive {
    pub name: String,
    pub positional: Vec<Value>,
    pub named: BTreeMap<String, Value>,
}
impl Directive {
    pub fn strings(&self) -> Result<Vec<&str>> {
        if !self.named.is_empty() || self.positional.is_empty() {
            return Err(Error::invalid(format!(
                "${} requires positional string arguments",
                self.name
            )));
        }
        self.positional
            .iter()
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    Error::invalid(format!("${} requires string arguments", self.name))
                })
            })
            .collect()
    }
    pub fn one(&self) -> Result<&str> {
        let args = self.strings()?;
        if args.len() != 1 {
            return Err(Error::invalid(format!(
                "${} expects one argument",
                self.name
            )));
        }
        Ok(args[0])
    }
    pub fn flag(&self) -> Result<()> {
        if !self.positional.is_empty() || !self.named.is_empty() {
            return Err(Error::invalid(format!("${} takes no arguments", self.name)));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryPreference {
    pub negative: bool,
    pub text: Option<String>,
    pub recent: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuerySyntax {
    pub intent: String,
    pub cues: Vec<QueryCue>,
    pub directives: Vec<Directive>,
    pub preferences: Vec<QueryPreference>,
}
impl QuerySyntax {
    pub fn canonical(&self) -> String {
        let mut parts = vec![
            self.intent
                .replace('\\', "\\\\")
                .replace('$', "\\$")
                .replace('@', "\\@")
                .replace('#', "\\#"),
        ];
        let quote = |value: &str| serde_json::to_string(value).expect("text serializes");
        for cue in &self.cues {
            parts.push(match cue {
                QueryCue::Concept { text } => format!("#({})", quote(text)),
                QueryCue::Selector { selector, locators } => format!(
                    "@{}({})",
                    selector.name(),
                    locators
                        .iter()
                        .map(|value| quote(value))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            });
        }
        for directive in &self.directives {
            let args = directive
                .positional
                .iter()
                .map(Value::to_string)
                .chain(
                    directive
                        .named
                        .iter()
                        .map(|(key, value)| format!("{key}={value}")),
                )
                .collect::<Vec<_>>()
                .join(",");
            parts.push(if args.is_empty() {
                format!("${}", directive.name)
            } else {
                format!("${}({args})", directive.name)
            });
        }
        for preference in &self.preferences {
            let value = preference
                .text
                .as_ref()
                .map(|value| quote(value))
                .unwrap_or_else(|| {
                    format!(
                        "recent,{}",
                        preference.recent.as_deref().unwrap_or("recorded")
                    )
                });
            parts.push(format!(
                "${}({value})",
                if preference.negative {
                    "avoid"
                } else {
                    "prefer"
                }
            ));
        }
        parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}
