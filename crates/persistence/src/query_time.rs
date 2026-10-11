// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Timelike, Utc};
use nous_core::{Error, Result, TimePredicate};
use sqlx::{Postgres, QueryBuilder};

/// Column names are supplied by the semantic owner's static query, never wire input.
pub enum TimeColumns {
    Instant(&'static str),
    Extent {
        kind: &'static str,
        start: &'static str,
        end: &'static str,
    },
}

/// Encode the Core predicate as SQL before candidate budgets are applied.
pub fn push_time_predicate(
    sql: &mut QueryBuilder<Postgres>,
    columns: TimeColumns,
    predicate: TimePredicate,
) -> Result<()> {
    use TimePredicate::{Point, Range};
    sql.push(" AND (");
    match columns {
        TimeColumns::Instant(column) => match predicate {
            Point { at } => {
                if at.nanosecond().is_multiple_of(1000) {
                    sql.push(column).push("=").push_bind(at);
                } else {
                    sql.push("FALSE");
                }
            }
            Range { start, end } => {
                sql.push(column).push(" IS NOT NULL");
                if let Some(start) = start {
                    sql.push(" AND ")
                        .push(column)
                        .push(">=")
                        .push_bind(ceil_micro(start)?);
                }
                if let Some(end) = end {
                    sql.push(" AND ")
                        .push(column)
                        .push("<")
                        .push_bind(ceil_micro(end)?);
                }
            }
        },
        TimeColumns::Extent { kind, start, end } => {
            sql.push("(").push(kind).push("='instant'");
            match predicate {
                Point { at } => {
                    if at.nanosecond().is_multiple_of(1000) {
                        sql.push(" AND ").push(start).push("=").push_bind(at);
                    } else {
                        sql.push(" AND FALSE");
                    }
                }
                Range {
                    start: from,
                    end: to,
                } => {
                    if let Some(from) = from {
                        sql.push(" AND ")
                            .push(start)
                            .push(">=")
                            .push_bind(ceil_micro(from)?);
                    }
                    if let Some(to) = to {
                        sql.push(" AND ")
                            .push(start)
                            .push("<")
                            .push_bind(ceil_micro(to)?);
                    }
                }
            }
            sql.push(") OR (").push(kind).push("='interval'");
            match predicate {
                Point { at } => {
                    sql.push(" AND (")
                        .push(start)
                        .push(" IS NULL OR ")
                        .push(start)
                        .push("<=")
                        .push_bind(floor_micro(at))
                        .push(")");
                    sql.push(" AND (")
                        .push(end)
                        .push(" IS NULL OR ")
                        .push(end)
                        .push(">")
                        .push_bind(floor_micro(at))
                        .push(")");
                }
                Range {
                    start: from,
                    end: to,
                } => {
                    if let Some(from) = from {
                        sql.push(" AND (")
                            .push(end)
                            .push(" IS NULL OR ")
                            .push(end)
                            .push(">")
                            .push_bind(floor_micro(from))
                            .push(")");
                    }
                    if let Some(to) = to {
                        sql.push(" AND (")
                            .push(start)
                            .push(" IS NULL OR ")
                            .push(start)
                            .push("<")
                            .push_bind(ceil_micro(to)?)
                            .push(")");
                    }
                }
            }
            sql.push(")");
        }
    }
    sql.push(")");
    Ok(())
}

// PostgreSQL stores microseconds. Directed rounding encodes the original
// nanosecond comparison exactly on that grid; the bound query stays untouched.
fn floor_micro(at: DateTime<Utc>) -> DateTime<Utc> {
    at.with_nanosecond(at.nanosecond() / 1000 * 1000)
        .expect("same-second microsecond")
}
fn ceil_micro(at: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let floor = floor_micro(at);
    if floor == at {
        return Ok(at);
    }
    floor
        .checked_add_signed(chrono::Duration::microseconds(1))
        .ok_or_else(|| Error::Invalid("time exceeds database range".into()))
}
