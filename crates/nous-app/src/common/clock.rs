//! Real time and deadlines. Cognitive time semantics remain in their domain.
use chrono::{DateTime, Utc};
use nous_domain::TimePoint;
use std::time::Instant;

#[derive(Clone, Copy, Default)]
pub struct Clock;
impl Clock {
    pub fn utc(&self) -> DateTime<Utc> {
        Utc::now()
    }
    pub fn recorded(&self) -> String {
        self.utc().to_rfc3339()
    }
    pub fn monotonic(&self) -> Instant {
        Instant::now()
    }
    pub fn capture(&self) -> TimePoint {
        TimePoint {
            domain: "unix_ms".into(),
            coordinate: self.utc().timestamp_millis(),
        }
    }
    pub fn budget_day(&self) -> String {
        self.utc().format("%Y-%m-%d").to_string()
    }
}
