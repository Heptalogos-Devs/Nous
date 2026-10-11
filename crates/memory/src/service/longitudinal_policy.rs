// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_configuration::*;
use nous_core::{Error, Result};
pub const JOURNAL_MAX_EPISODES: ConfigKey<u64> = ConfigKey::new("journal.max_episode_count");
pub const JOURNAL_MAX_SPAN: ConfigKey<u64> = ConfigKey::new("journal.max_span_seconds");
pub const CONSOLIDATION_DELAY: ConfigKey<u64> =
    ConfigKey::new("consolidation.settle_delay_seconds");

pub const CONSOLIDATION_MAX_ACTIONS: ConfigKey<u64> = ConfigKey::new("consolidation.max_actions");

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationContextPolicy {
    #[schemars(range(min = 1, max = 8192))]
    pub query_cue_chars: usize,
    #[schemars(range(min = 1, max = 16384))]
    pub candidate_text_chars: usize,
    #[schemars(range(min = 1, max = 64))]
    pub candidate_limit: usize,
    #[schemars(range(min = 1, max = 512))]
    pub basis_limit: usize,
    #[schemars(range(min = 1, max = 512))]
    pub provenance_root_limit: usize,
    #[schemars(range(min = 1, max = 128))]
    pub entity_limit: usize,
}
pub const EPISODE_SYNOPSIS: ConfigKey<nous_persistence::EpisodeTextBudget> =
    ConfigKey::new("episode.synopsis");
pub const CONSOLIDATION_CONTEXT: ConfigKey<ConsolidationContextPolicy> =
    ConfigKey::new("consolidation.context");

pub fn register_longitudinal_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    let reference = ReferenceProfile::parse(include_str!(
        "../../../../config/reference/longitudinal.json"
    ))?;
    registry.register(
        EPISODE_SYNOPSIS,
        "memory",
        "Episode readable synopsis and Serving member text budgets.",
        reference.get(EPISODE_SYNOPSIS)?,
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::ServingRebuild,
        ConfigSemanticEffect::ServingProjection,
        |_: &nous_persistence::EpisodeTextBudget| Ok(()),
    )?;
    reference.tag(registry, EPISODE_SYNOPSIS.path())?;
    registry.register(
        CONSOLIDATION_CONTEXT,
        "memory",
        "Longitudinal consolidation retrieval and model input budgets.",
        reference.get(CONSOLIDATION_CONTEXT)?,
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |_: &ConsolidationContextPolicy| Ok(()),
    )?;
    reference.tag(registry, CONSOLIDATION_CONTEXT.path())?;
    registry.register(
        JOURNAL_MAX_EPISODES,
        "memory",
        "Maximum Episodes in a Journal synthesis scope.",
        reference.get(JOURNAL_MAX_EPISODES)?,
        ConfigExposure::Advanced,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |value| {
            if (1..=64).contains(value) {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "Journal scope must contain 1..64 Episodes".into(),
                ))
            }
        },
    )?;
    registry.register(
        JOURNAL_MAX_SPAN,
        "memory",
        "Maximum Journal experience span in cognitive seconds.",
        reference.get(JOURNAL_MAX_SPAN)?,
        ConfigExposure::Advanced,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |value| {
            if (1..=604800).contains(value) {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "Journal span must be 1..604800 seconds".into(),
                ))
            }
        },
    )?;
    registry.register(
        CONSOLIDATION_DELAY,
        "memory",
        "Settling delay before longitudinal consolidation in cognitive seconds.",
        reference.get(CONSOLIDATION_DELAY)?,
        ConfigExposure::Advanced,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |value| {
            if (1..=86400).contains(value) {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "Consolidation delay must be 1..86400 seconds".into(),
                ))
            }
        },
    )?;
    registry.register(
        CONSOLIDATION_MAX_ACTIONS,
        "memory",
        "Maximum actions in one atomic longitudinal consolidation proposal.",
        reference.get(CONSOLIDATION_MAX_ACTIONS)?,
        ConfigExposure::Advanced,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |value| {
            if (1..=16).contains(value) {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "Consolidation must contain 1..16 actions".into(),
                ))
            }
        },
    )?;
    registry.bounds(JOURNAL_MAX_EPISODES, 1, 64, Some("items"))?;
    registry.bounds(JOURNAL_MAX_SPAN, 1, 604800, Some("cognitive_seconds"))?;
    registry.bounds(CONSOLIDATION_DELAY, 1, 86400, Some("cognitive_seconds"))?;
    registry.bounds(CONSOLIDATION_MAX_ACTIONS, 1, 16, Some("items"))?;
    reference.tag(registry, JOURNAL_MAX_EPISODES.path())?;
    reference.tag(registry, JOURNAL_MAX_SPAN.path())?;
    reference.tag(registry, CONSOLIDATION_DELAY.path())?;
    reference.tag(registry, CONSOLIDATION_MAX_ACTIONS.path())?;
    Ok(())
}
