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
