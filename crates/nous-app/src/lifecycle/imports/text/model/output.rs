use nous_domain::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportTriple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
}
impl ImportTriple {
    fn validate(&self) -> Result<()> {
        for value in [&self.subject, &self.predicate, &self.object] {
            text(value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NarrativeExtraction {
    pub events: Vec<String>,
    pub relations: Vec<ImportTriple>,
}
impl NarrativeExtraction {
    pub fn validate(&self) -> Result<()> {
        for event in &self.events {
            text(event)?;
        }
        for relation in &self.relations {
            relation.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactualExtraction {
    pub triples: Vec<ImportTriple>,
    pub entities: Vec<String>,
}
impl FactualExtraction {
    pub fn validate(&self) -> Result<()> {
        for entity in &self.entities {
            text(entity)?;
        }
        for triple in &self.triples {
            triple.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeExtraction {
    pub event_time: Option<String>,
    pub event_time_start: Option<String>,
    pub event_time_end: Option<String>,
    pub time_range: Option<[String; 2]>,
    pub time_granularity: Option<String>,
    pub time_confidence: Option<f64>,
}
impl TimeExtraction {
    pub fn validate(&self) -> Result<()> {
        for value in [
            &self.event_time,
            &self.event_time_start,
            &self.event_time_end,
            &self.time_granularity,
        ]
        .into_iter()
        .flatten()
        {
            text(value)?;
        }
        if let Some(range) = &self.time_range {
            for value in range {
                text(value)?;
            }
        }
        if self
            .time_confidence
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(Error::invalid(
                "time extraction confidence must be finite and within 0..1",
            ));
        }
        Ok(())
    }
}

fn text(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(Error::invalid(
            "extracted text must be nonempty and contain no NUL",
        ));
    }
    Ok(())
}
