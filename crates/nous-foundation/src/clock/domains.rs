use crate::configuration::{ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy};
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, ErrorCode, Result, validate_name};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const TIME_DOMAINS: ConfigKey<TimeDomains> = ConfigKey::new("time.domains");
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimeDomainScale {
    Linear { ticks: u64, seconds: u64 },
    Uncalibrated,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct TimeDomains(pub BTreeMap<String, TimeDomainScale>);
impl Default for TimeDomains {
    fn default() -> Self {
        Self(BTreeMap::from([(
            "unix_ms".into(),
            TimeDomainScale::Linear {
                ticks: 1000,
                seconds: 1,
            },
        )]))
    }
}
impl TimeDomains {
    pub fn validate(&self) -> Result<()> {
        for (domain, scale) in &self.0 {
            validate_name(domain, "time domain")?;
            if matches!(
                scale,
                TimeDomainScale::Linear { ticks: 0, .. }
                    | TimeDomainScale::Linear { seconds: 0, .. }
            ) {
                return Err(Error::invalid(
                    "linear time scales require positive ticks and seconds",
                ));
            }
        }
        if !matches!(self.0.get("unix_ms"),Some(TimeDomainScale::Linear{ticks,seconds})if u128::from(*ticks)==u128::from(*seconds)*1000)
        {
            return Err(Error::invalid("unix_ms has exactly 1000 ticks per second"));
        }
        Ok(())
    }
    /// Integer coordinate lower bounds use floor(duration/tick); no wider window is admitted.
    pub fn duration_ticks(&self, domain: &str, milliseconds: i64) -> Result<i64> {
        if milliseconds <= 0 {
            return Err(Error::invalid("relative time duration must be positive"));
        }
        let scale = self.0.get(domain).ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "time domain has no registered coordinate scale",
            )
            .with_context("domain", domain)
        })?;
        let TimeDomainScale::Linear { ticks, seconds } = scale else {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "relative duration requires a calibrated linear time domain",
            )
            .with_context("domain", domain));
        };
        let numerator = (milliseconds as u128)
            .checked_mul(u128::from(*ticks))
            .ok_or_else(|| Error::invalid("time duration conversion overflow"))?;
        let denominator = u128::from(*seconds)
            .checked_mul(1000)
            .filter(|value| *value > 0)
            .ok_or_else(|| Error::invalid("invalid time coordinate scale"))?;
        i64::try_from(numerator / denominator)
            .map_err(|_| Error::invalid("time duration exceeds the coordinate range"))
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(TIME_DOMAINS,ConfigDefinition{owner:"time".into(),description:"Named coordinate domains and exact linear duration scales; hosts own actual cognitive now points.".into(),default:TimeDomains::default(),scopes:vec![ConfigLevel::Instance,ConfigLevel::Namespace,ConfigLevel::Subject],apply_mode:ConfigApplyMode::Live,write_policy:ConfigWritePolicy::Scoped},TimeDomains::validate)
}
