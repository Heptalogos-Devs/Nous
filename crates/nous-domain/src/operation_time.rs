//! One infrastructure UTC instant; cognitive domain times remain explicit domain values.
use crate::TimePoint;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationTime(DateTime<Utc>);

impl OperationTime {
    pub fn new(utc: DateTime<Utc>) -> Self {
        Self(utc)
    }
    pub fn utc(&self) -> DateTime<Utc> {
        self.0
    }
    pub fn recorded(&self) -> String {
        self.0.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
    }
    pub fn unix_ms(&self) -> i64 {
        self.0.timestamp_millis()
    }
    pub fn point(&self) -> TimePoint {
        TimePoint {
            domain: "unix_ms".into(),
            coordinate: self.unix_ms(),
        }
    }
}
