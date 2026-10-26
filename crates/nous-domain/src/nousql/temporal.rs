use super::Directive;
use crate::{Error, OperationTime, Result, TimeAxis, TimeFilter, TimePoint, TimeRange};
use serde_json::Value;

pub fn duration_ms(value: &str) -> Result<i64> {
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| Error::invalid("duration requires ms,s,m,h,d or w"))?;
    let amount = value[..split]
        .parse::<i64>()
        .map_err(|error| Error::invalid("invalid duration").with_cause(&error))?;
    let multiplier = match &value[split..] {
        "ms" => 1,
        "s" => 1000,
        "m" => 60000,
        "h" => 3600000,
        "d" => 86400000,
        "w" => 604800000,
        _ => return Err(Error::invalid("unknown duration unit")),
    };
    amount
        .checked_mul(multiplier)
        .filter(|value| *value > 0)
        .ok_or_else(|| Error::invalid("duration must be positive and fit milliseconds"))
}
pub fn coordinate(value: &Value) -> Result<i64> {
    if let Some(number) = value.as_i64() {
        return Ok(number);
    }
    let value = value.as_str().ok_or_else(|| {
        Error::invalid("time requires a coordinate or timezone-bearing timestamp")
    })?;
    OperationTime::parse(value).map(|time| time.unix_ms())
}
fn coordinate_in(value: &Value, domain: &str) -> Result<i64> {
    if domain != "unix_ms" && !value.is_number() {
        return Err(Error::new(crate::ErrorCode::CapabilityUnavailable,"calendar timestamps require the domain's calendar mapping; named coordinates accept exact integers").with_context("domain",domain));
    }
    coordinate(value)
}
pub fn time_filter(
    directive: &Directive,
    clock: &TimePoint,
    recorded: &TimePoint,
    relative_ticks: Option<i64>,
) -> Result<Option<TimeFilter>> {
    if directive.positional.len() != 1 {
        return Err(Error::invalid("$time requires one time axis"));
    }
    let axis = match directive.positional[0].as_str() {
        Some("occurred" | "event") => TimeAxis::Event,
        Some("observed" | "observation") => TimeAxis::Observation,
        Some("formed" | "formation") => TimeAxis::Formation,
        Some("recorded") => TimeAxis::Recorded,
        Some("valid" | "validity") => TimeAxis::Validity,
        _ => return Err(Error::invalid("unknown time axis")),
    };
    let now = if matches!(axis, TimeAxis::Recorded) {
        recorded
    } else {
        clock
    };
    let domain = directive
        .named
        .get("domain")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| Error::invalid("time domain must be a string"))
        })
        .transpose()?
        .unwrap_or(&now.domain)
        .to_owned();
    if matches!(axis, TimeAxis::Recorded) && domain != "unix_ms" {
        return Err(Error::invalid("recorded time uses unix_ms"));
    }
    if directive
        .named
        .keys()
        .any(|key| !["domain", "at", "from", "to", "within"].contains(&key.as_str()))
    {
        return Err(Error::invalid("unknown $time argument"));
    }
    let keys = directive
        .named
        .keys()
        .filter(|key| key.as_str() != "domain")
        .count();
    let (start, end) = if let Some(at) = directive.named.get("at") {
        if keys != 1 {
            return Err(Error::invalid("time at cannot combine with other bounds"));
        }
        let point = coordinate_in(at, &domain)?;
        (Some(point), Some(point))
    } else if let Some(within) = directive.named.get("within") {
        if keys != 1 || domain != now.domain {
            return Err(Error::invalid(
                "relative time needs one window in the captured clock domain",
            ));
        }
        let _ = within
            .as_str()
            .ok_or_else(|| Error::invalid("within requires a duration"))?;
        let duration = relative_ticks.ok_or_else(|| {
            Error::new(
                crate::ErrorCode::CapabilityUnavailable,
                "relative time needs a registered domain scale",
            )
        })?;
        if duration == 0 {
            return Ok(None);
        }
        (
            Some(
                now.coordinate
                    .checked_sub(duration)
                    .ok_or_else(|| Error::invalid("time window overflow"))?,
            ),
            Some(
                now.coordinate
                    .checked_sub(1)
                    .ok_or_else(|| Error::invalid("time endpoint overflow"))?,
            ),
        )
    } else {
        if keys == 0 {
            return Err(Error::invalid("$time requires at, within, from or to"));
        }
        let start = directive
            .named
            .get("from")
            .map(|value| coordinate_in(value, &domain))
            .transpose()?;
        let exclusive = directive
            .named
            .get("to")
            .map(|value| coordinate_in(value, &domain))
            .transpose()?;
        if start
            .zip(exclusive)
            .is_some_and(|(start, end)| start >= end)
        {
            return Err(Error::invalid("time range requires from < to"));
        }
        (
            start,
            exclusive
                .map(|end| {
                    end.checked_sub(1)
                        .ok_or_else(|| Error::invalid("time endpoint overflow"))
                })
                .transpose()?,
        )
    };
    let filter = TimeFilter {
        axis,
        range: TimeRange { domain, start, end },
    };
    filter.validate()?;
    Ok(Some(filter))
}
