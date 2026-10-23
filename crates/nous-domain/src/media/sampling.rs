//! Actual preprocessing coverage from the upstream bounded video sampling path.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;

/// Runtime sampling facts are recorded separately from a model's interpretation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivationQuality {
    pub partial: bool,
    pub source_duration_ms: Option<u64>,
    pub sampling: Option<FrameSamplingReport>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameSamplingReport {
    pub implementation: String,
    pub implementation_version: String,
    pub policy: String,
    pub sampled_duration_ms: u64,
    pub requested_timestamps_ms: Vec<u64>,
    pub sampled_timestamps_ms: Vec<u64>,
    pub omitted_timestamps_ms: Vec<u64>,
    pub audio_included: bool,
    pub audio_omitted: bool,
}
impl DerivationQuality {
    pub fn validate(&self, duration_ms: Option<u64>) -> Result<()> {
        if let (Some(recorded), Some(actual)) = (self.source_duration_ms, duration_ms)
            && recorded != actual
        {
            return Err(Error::invalid(
                "derivation duration differs from admitted source metadata",
            ));
        }
        if let Some(report) = &self.sampling {
            if !self.partial {
                return Err(Error::invalid(
                    "frame sampling retains partial visual coverage",
                ));
            }
            bounded_text(&report.implementation, 256, "sampling implementation", true)?;
            bounded_text(
                &report.implementation_version,
                256,
                "sampling version",
                true,
            )?;
            bounded_text(&report.policy, 128, "sampling policy", true)?;
            let source_duration = duration_ms
                .or(self.source_duration_ms)
                .ok_or_else(|| Error::invalid("sampling requires actual source duration"))?;
            if report.sampled_duration_ms == 0
                || report.sampled_duration_ms > source_duration
                || report.requested_timestamps_ms.is_empty()
                || report.requested_timestamps_ms.len() > 64
                || report.sampled_timestamps_ms.is_empty()
                || report.sampled_timestamps_ms.len() + report.omitted_timestamps_ms.len()
                    != report.requested_timestamps_ms.len()
                || (report.audio_included && report.audio_omitted)
            {
                return Err(Error::invalid(
                    "sampling counts, duration or audio coverage are inconsistent",
                ));
            }
            for times in [
                &report.requested_timestamps_ms,
                &report.sampled_timestamps_ms,
                &report.omitted_timestamps_ms,
            ] {
                if times.iter().any(|at| *at > report.sampled_duration_ms)
                    || times.windows(2).any(|pair| pair[0] > pair[1])
                {
                    return Err(Error::invalid(
                        "sampling timeline lies outside its admitted source span",
                    ));
                }
            }
            let mut requested = std::collections::BTreeMap::new();
            for at in &report.requested_timestamps_ms {
                *requested.entry(at).or_insert(0usize) += 1;
            }
            for at in &report.omitted_timestamps_ms {
                let count = requested
                    .get_mut(at)
                    .ok_or_else(|| Error::invalid("omitted sampling time was not requested"))?;
                if *count == 0 {
                    return Err(Error::invalid("omitted sampling time is repeated"));
                }
                *count -= 1;
            }
            if (!report.omitted_timestamps_ms.is_empty()
                || report.sampled_duration_ms < source_duration
                || report.audio_omitted)
                && !self.partial
            {
                return Err(Error::invalid(
                    "partial sampling must preserve its coverage limitation",
                ));
            }
        }
        Ok(())
    }
}
