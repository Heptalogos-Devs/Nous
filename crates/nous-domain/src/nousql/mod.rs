mod parser;
pub const MANUAL: &str = include_str!("manual.md");
pub use parser::parse;
mod temporal;
use crate::{QueryRequest, RetrievalTarget, TimePoint};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub use temporal::{coordinate as time_coordinate, duration_ms, time_filter};

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
    pub fn strings(&self) -> crate::Result<Vec<&str>> {
        if !self.named.is_empty() || self.positional.is_empty() {
            return Err(crate::Error::invalid(format!(
                "${} requires positional string arguments",
                self.name
            )));
        }
        self.positional
            .iter()
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    crate::Error::invalid(format!("${} requires string arguments", self.name))
                })
            })
            .collect()
    }
    pub fn one(&self) -> crate::Result<&str> {
        let args = self.strings()?;
        if args.len() != 1 {
            return Err(crate::Error::invalid(format!(
                "${} expects one argument",
                self.name
            )));
        }
        Ok(args[0])
    }
    pub fn flag(&self) -> crate::Result<()> {
        if !self.positional.is_empty() || !self.named.is_empty() {
            return Err(crate::Error::invalid(format!(
                "${} takes no arguments",
                self.name
            )));
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundSelector {
    pub selector: SelectorKind,
    pub canonical: Vec<String>,
    pub terms: Vec<String>,
    pub targets: Vec<RetrievalTarget>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedQuery {
    pub effort: String,
    pub direct_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<QueryLeafStats>,
    pub request: QueryRequest,
    pub canonical: String,
    pub selectors: Vec<BoundSelector>,
    pub preferences: Vec<QueryPreference>,
    pub explore: bool,
    pub materialize: bool,
    pub diagnostics: String,
    pub candidate_budget: usize,
    pub query_sources: Vec<crate::SourceReadRef>,
    pub current_view: bool,
    pub empty: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum PlannedExpression {
    Leaf { index: usize },
    Any { children: Vec<PlannedExpression> },
    All { children: Vec<PlannedExpression> },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPlan {
    pub rerank: crate::QueryRerankMode,
    pub sort: Option<crate::QuerySort>,
    pub time_units: BTreeMap<String, Value>,
    pub source: String,
    pub canonical: String,
    pub authority_commit_seq: i64,
    pub clock: TimePoint,
    pub recorded_at: String,
    pub expression: PlannedExpression,
    pub leaves: Vec<PlannedQuery>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryLeafStats {
    pub eligible_objects: usize,
    pub offered_objects: usize,
    pub truncated: bool,
    pub semantic: Option<crate::SemanticQueryInfo>,
}
