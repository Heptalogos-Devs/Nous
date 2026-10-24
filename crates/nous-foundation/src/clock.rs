//! Real time and deadlines. Cognitive time semantics remain in their domain.
use chrono::{DateTime, Utc};
use nous_domain::{OperationTime, TimePoint};
use std::time::Instant;

#[derive(Clone, Copy, Default)]
pub struct Clock;
impl Clock {
    pub fn operation_snapshot() -> OperationTime {
        OperationTime::new(Utc::now())
    }
    pub fn utc(&self) -> DateTime<Utc> {
        Self::operation_snapshot().utc()
    }
    pub fn recorded(&self) -> String {
        Self::operation_snapshot().recorded()
    }
    pub fn monotonic(&self) -> Instant {
        Instant::now()
    }
    pub fn capture(&self) -> TimePoint {
        Self::operation_snapshot().point()
    }
}
