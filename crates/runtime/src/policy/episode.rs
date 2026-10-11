//! Cheap typed boundary signals; Episode identity belongs to Memory.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Duration, Utc};
use nous_core::{Result, SessionId};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperienceContext {
    pub session: SessionId,
    pub work_context: Option<Uuid>,
    pub conversation: Option<String>,
    pub actor: Option<String>,
    pub source_class: String,
}

#[derive(Debug, Clone)]
pub struct EpisodePolicy {
    pub soft_idle: Duration,
    pub hard_idle: Duration,
    pub context_switch_count: usize,
}

impl EpisodePolicy {
    pub fn from_snapshot(snapshot: &nous_configuration::ConfigSnapshot) -> Result<Self> {
        Ok(Self {
            soft_idle: Duration::seconds(snapshot.get(SOFT_IDLE_KEY)? as i64),
            hard_idle: Duration::seconds(snapshot.get(HARD_IDLE_KEY)? as i64),
            context_switch_count: snapshot.get(CONTEXT_SWITCH_COUNT)?,
        })
    }

    pub fn boundary(
        &self,
        previous: &ExperienceContext,
        next: &ExperienceContext,
        previous_at: DateTime<Utc>,
        next_at: DateTime<Utc>,
        work_context_ended: bool,
    ) -> Option<&'static str> {
        if work_context_ended {
            return Some("work_context_ended");
        }
        let gap = next_at - previous_at;
        if gap >= self.hard_idle {
            return Some("hard_idle");
        }
        let switches = usize::from(previous.work_context != next.work_context)
            + usize::from(previous.conversation != next.conversation)
            + usize::from(previous.actor != next.actor)
            + usize::from(previous.source_class != next.source_class);
        if switches >= self.context_switch_count {
            Some("context_switch")
        } else if gap >= self.soft_idle && (switches > 0 || previous.session != next.session) {
            Some("soft_idle_context_switch")
        } else {
            None
        }
    }
}

pub const SOFT_IDLE_KEY: nous_configuration::ConfigKey<u64> =
    nous_configuration::ConfigKey::new("episode.soft_idle_seconds");
pub const HARD_IDLE_KEY: nous_configuration::ConfigKey<u64> =
    nous_configuration::ConfigKey::new("episode.hard_idle_seconds");
pub const SETTLE_DELAY_KEY: nous_configuration::ConfigKey<u64> =
    nous_configuration::ConfigKey::new("episode.settle_delay_seconds");

pub const CONTEXT_SWITCH_COUNT: nous_configuration::ConfigKey<usize> =
    nous_configuration::ConfigKey::new("episode.context_switch_count");

pub fn register_episode_configuration(
    registry: &mut nous_configuration::ConfigRegistryBuilder,
) -> Result<()> {
    use nous_configuration::*;
    let reference = ReferenceProfile::parse(include_str!(
        "../../../../config/reference/longitudinal.json"
    ))?;
    for (key, description) in [
        (
            SOFT_IDLE_KEY,
            "Soft Episode idle threshold in cognitive seconds.",
        ),
        (
            HARD_IDLE_KEY,
            "Hard Episode idle closure in cognitive seconds.",
        ),
        (
            SETTLE_DELAY_KEY,
            "Cognitive settling delay before semantic Episode review.",
        ),
    ] {
        registry.register(
            key,
            "cognitive-runtime",
            description,
            reference.get(key)?,
            ConfigExposure::Advanced,
            ConfigScopePolicy::SubjectOverrideAllowed,
            ConfigApplyMode::Live,
            ConfigSemanticEffect::AuthorityFormation,
            |value| {
                if (1..=86400).contains(value) {
                    Ok(())
                } else {
                    Err(nous_core::Error::Invalid(
                        "Episode interval must be 1..86400 seconds".into(),
                    ))
                }
            },
        )?;
        registry.bounds(key, 1, 86400, Some("cognitive_seconds"))?;
        reference.tag(registry, key.path())?;
    }
    registry.register(
        CONTEXT_SWITCH_COUNT,
        "cognitive-runtime",
        "Context dimensions required for an Episode boundary.",
        reference.get(CONTEXT_SWITCH_COUNT)?,
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |value| {
            if (1..=4).contains(value) {
                Ok(())
            } else {
                Err(nous_core::Error::Invalid(
                    "context switch count must be 1..4".into(),
                ))
            }
        },
    )?;
    registry.bounds(CONTEXT_SWITCH_COUNT, 1, 4, Some("items"))?;
    reference.tag(registry, CONTEXT_SWITCH_COUNT.path())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuity_and_idle_context_boundaries() {
        let policy = EpisodePolicy {
            soft_idle: Duration::minutes(5),
            hard_idle: Duration::minutes(30),
            context_switch_count: 2,
        };
        let at = DateTime::<Utc>::UNIX_EPOCH;
        let previous = ExperienceContext {
            session: SessionId::new(),
            work_context: None,
            conversation: Some("chat:a".into()),
            actor: None,
            source_class: "message".into(),
        };
        let mut next = previous.clone();
        next.session = SessionId::new();
        assert_eq!(
            policy.boundary(&previous, &next, at, at + Duration::minutes(1), false),
            None
        );
        assert_eq!(
            policy.boundary(&previous, &next, at, at + Duration::minutes(5), false),
            Some("soft_idle_context_switch")
        );
        next = previous.clone();
        assert_eq!(
            policy.boundary(&previous, &next, at, at + Duration::minutes(30), false),
            Some("hard_idle")
        );
        next.actor = Some("person:b".into());
        next.conversation = Some("chat:b".into());
        assert_eq!(
            policy.boundary(&previous, &next, at, at, false),
            Some("context_switch")
        );
        assert_eq!(
            policy.boundary(&previous, &previous, at, at, true),
            Some("work_context_ended")
        );
    }
}
