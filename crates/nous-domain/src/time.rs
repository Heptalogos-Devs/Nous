//! Shared coordinates and ranges; named time domains own their interpretation.
use crate::{Error, Result, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimePoint {
    pub domain: String,
    pub coordinate: i64,
}
impl TimePoint {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.domain, "time domain")
    }
    pub fn compare(&self, other: &Self) -> Result<std::cmp::Ordering> {
        if self.domain != other.domain {
            return Err(Error::invalid("time points require a common time domain"));
        }
        Ok(self.coordinate.cmp(&other.coordinate))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TimeRange {
    pub domain: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
}
impl TimeRange {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.domain, "time domain")?;
        if matches!((self.start, self.end), (Some(a), Some(b)) if a > b) {
            return Err(Error::invalid("time range starts after it ends"));
        }
        Ok(())
    }
    pub fn contains(&self, value: &TimePoint) -> bool {
        self.domain == value.domain
            && self.start.is_none_or(|start| value.coordinate >= start)
            && self.end.is_none_or(|end| value.coordinate <= end)
    }
    pub fn overlaps(&self, other: &Self) -> bool {
        self.domain == other.domain
            && self
                .start
                .zip(other.end)
                .is_none_or(|(start, end)| start <= end)
            && self
                .end
                .zip(other.start)
                .is_none_or(|(end, start)| end >= start)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CognitiveTimes {
    pub event: Option<TimePoint>,
    pub observation: Option<TimePoint>,
    pub formation: TimePoint,
    pub validity: Option<TimeRange>,
    pub recorded: TimePoint,
}
impl CognitiveTimes {
    pub fn validate(&self) -> Result<()> {
        self.formation.validate()?;
        self.recorded.validate()?;
        if let Some(value) = &self.event {
            value.validate()?;
        }
        if let Some(value) = &self.observation {
            value.validate()?;
        }
        if let Some(value) = &self.validity {
            value.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeAxis {
    Event,
    Observation,
    Formation,
    Recorded,
    Validity,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TimeFilter {
    pub axis: TimeAxis,
    pub range: TimeRange,
}

impl TimeFilter {
    pub fn validate(&self) -> Result<()> {
        self.range.validate()
    }
    pub fn matches(&self, times: &CognitiveTimes) -> bool {
        match self.axis {
            TimeAxis::Event => times
                .event
                .as_ref()
                .is_some_and(|time| self.range.contains(time)),
            TimeAxis::Observation => times
                .observation
                .as_ref()
                .is_some_and(|time| self.range.contains(time)),
            TimeAxis::Formation => self.range.contains(&times.formation),
            TimeAxis::Recorded => self.range.contains(&times.recorded),
            TimeAxis::Validity => times
                .validity
                .as_ref()
                .is_some_and(|range| self.range.overlaps(range)),
        }
    }
}
