use super::types::ExternalTime;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};
use nous_domain::*;

pub(super) fn point(value: &ExternalTime) -> Result<TimePoint> {
    let coordinate = match value {
        ExternalTime::Seconds(value) => {
            let value = value * 1000.0;
            if !value.is_finite() || value < i64::MIN as f64 || value >= i64::MAX as f64 {
                return Err(Error::invalid(
                    "external timestamp is outside the unix_ms domain",
                ));
            }
            value.round() as i64
        }
        ExternalTime::Text(value) => {
            if let Ok(time) = DateTime::parse_from_rfc3339(value) {
                time.timestamp_millis()
            } else {
                let local = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                    .or_else(|_| {
                        NaiveDate::parse_from_str(value, "%Y-%m-%d")
                            .map(|date| date.and_hms_opt(0, 0, 0).expect("valid midnight"))
                    })
                    .map_err(|error| {
                        Error::invalid(
                            "time needs Unix seconds, RFC3339, or a local YYYY-MM-DD date/time",
                        )
                        .with_cause(&error)
                    })?;
                Local.from_local_datetime(&local).single().ok_or_else(||Error::invalid("local timestamp is ambiguous or nonexistent; supply RFC3339 with an offset"))?.timestamp_millis()
            }
        }
    };
    Ok(TimePoint {
        domain: "unix_ms".into(),
        coordinate,
    })
}
pub(super) fn filter(
    start: Option<&ExternalTime>,
    end: Option<&ExternalTime>,
) -> Result<Option<TimeFilter>> {
    if start.is_none() && end.is_none() {
        return Ok(None);
    }
    let range = TimeRange {
        domain: "unix_ms".into(),
        start: start.map(point).transpose()?.map(|value| value.coordinate),
        end: end.map(point).transpose()?.map(|value| value.coordinate),
    };
    range.validate()?;
    Ok(Some(TimeFilter {
        axis: TimeAxis::Event,
        range,
    }))
}
