// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TemporalExtent {
    #[default]
    Unknown,
    Instant {
        at: DateTime<Utc>,
    },
    Interval {
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    },
}

impl TemporalExtent {
    /// Readable extent without changing its declared clock domain or open endpoints.
    pub fn context_text(&self) -> String {
        match self {
            Self::Unknown => "未确定".into(),
            Self::Instant { at } => at.to_rfc3339(),
            Self::Interval { start, end } => format!(
                "{} 至 {}（结束点不含）",
                start.map_or_else(|| "起点未限定".into(), |v| v.to_rfc3339()),
                end.map_or_else(|| "终点未限定".into(), |v| v.to_rfc3339()),
            ),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if let Self::Interval {
            start: Some(start),
            end: Some(end),
        } = self
            && start >= end
        {
            return Err(Error::Invalid(
                "temporal interval must be half-open with start < end".into(),
            ));
        }
        Ok(())
    }
}

/// A nonempty half-open range; omitted endpoints are unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TimeInterval {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

impl TimeInterval {
    pub fn validate(&self) -> Result<()> {
        if let (Some(start), Some(end)) = (self.start, self.end)
            && start >= end
        {
            return Err(Error::Invalid(
                "time interval must be half-open with start < end".into(),
            ));
        }
        Ok(())
    }

    pub fn contains(&self, value: DateTime<Utc>) -> bool {
        self.start.is_none_or(|start| value >= start) && self.end.is_none_or(|end| value < end)
    }

    pub fn overlaps(&self, start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> bool {
        let starts_before_end = match (self.start, end) {
            (Some(query_start), Some(value_end)) => query_start < value_end,
            _ => true,
        };
        let ends_after_start = match (self.end, start) {
            (Some(query_end), Some(value_start)) => value_start < query_end,
            _ => true,
        };
        starts_before_end && ends_after_start
    }
}

/// Query predicates distinguish exact points from nonempty half-open ranges.
/// Persistent temporal extents retain their own unknown/instant/interval meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TimePredicate {
    Point {
        at: DateTime<Utc>,
    },
    Range {
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    },
}

impl TimePredicate {
    pub fn validate(&self) -> Result<()> {
        match *self {
            Self::Point { .. } => Ok(()),
            Self::Range { start, end } => TimeInterval { start, end }.validate(),
        }
    }

    pub fn contains(&self, value: DateTime<Utc>) -> bool {
        match *self {
            Self::Point { at } => value == at,
            Self::Range { start, end } => TimeInterval { start, end }.contains(value),
        }
    }

    pub fn matches_extent(&self, extent: &TemporalExtent) -> bool {
        match extent {
            TemporalExtent::Unknown => false,
            TemporalExtent::Instant { at } => self.contains(*at),
            TemporalExtent::Interval { start, end } => match *self {
                Self::Point { at } => TimeInterval {
                    start: *start,
                    end: *end,
                }
                .contains(at),
                Self::Range {
                    start: from,
                    end: to,
                } => TimeInterval {
                    start: from,
                    end: to,
                }
                .overlaps(*start, *end),
            },
        }
    }

    /// Empty intersection is an ineligible query branch, not a zero-width range.
    pub fn intersection(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Point { at }, predicate) | (predicate, Self::Point { at }) => {
                predicate.contains(at).then_some(Self::Point { at })
            }
            (Self::Range { start: a, end: b }, Self::Range { start: c, end: d }) => {
                let result = Self::Range {
                    start: a.into_iter().chain(c).max(),
                    end: b.into_iter().chain(d).min(),
                };
                result.validate().ok().map(|()| result)
            }
        }
    }
}

/// An instant belongs to its named time domain. Recording time is always real UTC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainStamp {
    pub time_domain_ref: String,
    pub at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
}
impl DomainStamp {
    /// No inferred mapping between worlds, including numerically equal timestamps.
    pub fn compare(&self, other: &Self) -> Option<std::cmp::Ordering> {
        (!self.time_domain_ref.trim().is_empty() && self.time_domain_ref == other.time_domain_ref)
            .then(|| self.at.cmp(&other.at))
    }
}

/// A scoped extent does not compare against another world's numeric timestamps.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainTemporalExtent {
    pub time_domain_ref: String,
    pub extent: TemporalExtent,
}

impl DomainTemporalExtent {
    pub fn real(extent: TemporalExtent) -> Self {
        Self {
            time_domain_ref: "time:real".into(),
            extent,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.time_domain_ref.trim().is_empty() || self.time_domain_ref.len() > 256 {
            return Err(Error::Invalid("time domain requires 1..256 bytes".into()));
        }
        self.extent.validate()
    }
}
impl Default for DomainTemporalExtent {
    fn default() -> Self {
        Self::real(TemporalExtent::Unknown)
    }
}
/// A reference predicate has a domain; numeric equality across worlds is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainTimePredicate {
    pub time_domain_ref: String,
    pub predicate: TimePredicate,
}
impl DomainTimePredicate {
    pub fn validate(&self) -> Result<()> {
        if self.time_domain_ref.trim().is_empty() || self.time_domain_ref.len() > 256 {
            return Err(Error::Invalid(
                "time predicate domain requires 1..256 bytes".into(),
            ));
        }
        self.predicate.validate()
    }
    pub fn matches_extent(&self, value: &DomainTemporalExtent) -> Option<bool> {
        (self.time_domain_ref == value.time_domain_ref
            && !matches!(value.extent, TemporalExtent::Unknown))
        .then(|| self.predicate.matches_extent(&value.extent))
    }
    pub fn matches_stamp(&self, value: &DomainStamp) -> Option<bool> {
        if self.time_domain_ref == value.time_domain_ref {
            Some(self.predicate.contains(value.at))
        } else {
            None
        }
    }
}

pub fn real_time_domain() -> String {
    "time:real".into()
}
pub fn is_real_time_domain(value: &str) -> bool {
    value == "time:real"
}

/// Actual source rows, not independently aggregated axes which could invent a
/// witness by combining the event time of one contact with another observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceTemporalEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_class: Option<crate::SourceClass>,
    pub occurrence_id: crate::OccurrenceId,
    pub occurred: DomainTemporalExtent,
    pub observed: DomainStamp,
    pub observed_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
}
impl SourceTemporalEvidence {
    pub fn matches(
        &self,
        occurred: Option<&DomainTimePredicate>,
        observed: Option<&DomainTimePredicate>,
    ) -> bool {
        occurred.is_none_or(|p| p.matches_extent(&self.occurred) == Some(true))
            && observed.is_none_or(|p| {
                if p.time_domain_ref == "time:real" {
                    p.predicate.contains(self.observed_at)
                } else {
                    p.matches_stamp(&self.observed) == Some(true)
                }
            })
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemporalEvidence {
    pub sources: Vec<SourceTemporalEvidence>,
}
impl TemporalEvidence {
    pub fn matches(
        &self,
        occurred: Option<&DomainTimePredicate>,
        observed: Option<&DomainTimePredicate>,
    ) -> bool {
        (occurred.is_none() && observed.is_none())
            || self
                .sources
                .iter()
                .any(|source| source.matches(occurred, observed))
    }
    pub fn observed_real_at(&self) -> Option<DateTime<Utc>> {
        self.sources.iter().map(|source| source.observed_at).max()
    }
}
