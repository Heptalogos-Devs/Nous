// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_configuration::{
    ConfigApplyMode, ConfigExposure, ConfigKey, ConfigRegistryBuilder, ConfigScopePolicy,
    ConfigSemanticEffect, ConfigSnapshot,
};
use nous_persistence::database_error as db;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessibilityPolicy {
    pub epsilon: f64,
    pub tau_days: f64,
    pub decay: f64,
    pub formation_weight: f64,
    pub referenced_weight: f64,
    pub acted_on_weight: f64,
    pub result_supported_weight: f64,
    pub result_refuted_weight: f64,
    pub corrected_weight: f64,
    pub pinned_weight: f64,
    pub normal_threshold: f64,
    pub deep_threshold: f64,
}

impl Default for AccessibilityPolicy {
    fn default() -> Self {
        let reference = nous_configuration::ReferenceProfile::parse(include_str!(
            "../../../../config/reference/memory-accessibility.json"
        ))
        .expect("accessibility reference profile");
        Self {
            epsilon: reference.get(EPSILON).expect("reference epsilon"),
            tau_days: reference.get(TAU_DAYS).expect("reference tau_days"),
            decay: reference.get(DECAY).expect("reference decay"),
            formation_weight: reference
                .get(FORMATION_WEIGHT)
                .expect("reference formation_weight"),
            referenced_weight: reference
                .get(REFERENCED_WEIGHT)
                .expect("reference referenced_weight"),
            acted_on_weight: reference
                .get(ACTED_ON_WEIGHT)
                .expect("reference acted_on_weight"),
            result_supported_weight: reference
                .get(RESULT_SUPPORTED_WEIGHT)
                .expect("reference result_supported_weight"),
            result_refuted_weight: reference
                .get(RESULT_REFUTED_WEIGHT)
                .expect("reference result_refuted_weight"),
            corrected_weight: reference
                .get(CORRECTED_WEIGHT)
                .expect("reference corrected_weight"),
            pinned_weight: reference
                .get(PINNED_WEIGHT)
                .expect("reference pinned_weight"),
            normal_threshold: reference
                .get(NORMAL_THRESHOLD)
                .expect("reference normal_threshold"),
            deep_threshold: reference
                .get(DEEP_THRESHOLD)
                .expect("reference deep_threshold"),
        }
    }
}

impl AccessibilityPolicy {
    pub fn validate(self) -> Result<Self> {
        if self.epsilon <= 0.0
            || self.tau_days <= 0.0
            || self.decay <= 0.0
            || self.formation_weight < 0.0
            || !self.normal_threshold.is_finite()
            || !self.deep_threshold.is_finite()
            || self.normal_threshold <= self.deep_threshold
        {
            return Err(Error::Invalid(
                "invalid accessibility reference parameters".into(),
            ));
        }
        Ok(self)
    }

    pub fn activation(&self, age_days: f64, uses: &[(String, f64)]) -> f64 {
        let contribution = |weight: f64, delta: f64| {
            weight * (1.0 + delta.max(0.0) / self.tau_days).powf(-self.decay)
        };
        let mut mass = self.epsilon + contribution(self.formation_weight, age_days);
        for (kind, delta) in uses {
            mass += contribution(self.use_weight(kind), *delta);
        }
        mass.ln()
    }

    pub fn level_from_activation(&self, activation: f64) -> AccessibilityLevel {
        if activation >= self.normal_threshold {
            AccessibilityLevel::Normal
        } else if activation >= self.deep_threshold {
            AccessibilityLevel::Deep
        } else {
            AccessibilityLevel::Explicit
        }
    }

    pub async fn level_for_memory(
        &self,
        store: &AuthorityStore,
        subject: SubjectId,
        memory: MemoryId,
        now: DateTime<Utc>,
    ) -> Result<AccessibilityLevel> {
        let row = sqlx::query("SELECT accessibility_mode,created_at FROM memory_objects WHERE subject_id=$1 AND memory_id=$2")
            .bind(subject.0)
            .bind(memory.0)
            .fetch_optional(store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("memory not found".into()))?;
        let mode: AccessibilityMode = serde_json::from_value(serde_json::Value::String(
            row.try_get::<String, _>("accessibility_mode").map_err(db)?,
        ))
        .map_err(|_| Error::Infrastructure("invalid accessibility mode".into()))?;
        let created: DateTime<Utc> = row.try_get("created_at").map_err(db)?;
        if !matches!(mode, AccessibilityMode::Auto) {
            return Ok(match mode {
                AccessibilityMode::Normal => AccessibilityLevel::Normal,
                AccessibilityMode::Deep => AccessibilityLevel::Deep,
                AccessibilityMode::Explicit => AccessibilityLevel::Explicit,
                AccessibilityMode::Auto => unreachable!(),
            });
        }
        let uses = sqlx::query("SELECT use_kind,occurred_at FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='memory_revision' AND ref_value IN (SELECT memory_revision_id::text FROM memory_revisions WHERE memory_id=$2) AND use_kind <> 'presented'")
            .bind(subject.0)
            .bind(memory.0)
            .fetch_all(store.pool())
            .await
            .map_err(db)?
            .into_iter()
            .map(|row| {
                let kind: String = row.try_get("use_kind").map_err(db)?;
                let at: DateTime<Utc> = row.try_get("occurred_at").map_err(db)?;
                Ok((kind, (now - at).num_seconds().max(0) as f64 / 86400.0))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(self.level_from_activation(
            self.activation((now - created).num_seconds().max(0) as f64 / 86400.0, &uses),
        ))
    }

    fn use_weight(&self, kind: &str) -> f64 {
        match kind {
            "referenced" => self.referenced_weight,
            "acted_on" => self.acted_on_weight,
            "result_supported" => self.result_supported_weight,
            "result_refuted" => self.result_refuted_weight,
            "corrected" => self.corrected_weight,
            "pinned" => self.pinned_weight,
            "presented" => 0.0,
            _ => 0.0,
        }
    }
}

pub const EPSILON: ConfigKey<f64> = ConfigKey::new("memory.accessibility.epsilon");
pub const TAU_DAYS: ConfigKey<f64> = ConfigKey::new("memory.accessibility.tau_days");
pub const DECAY: ConfigKey<f64> = ConfigKey::new("memory.accessibility.decay");
pub const FORMATION_WEIGHT: ConfigKey<f64> =
    ConfigKey::new("memory.accessibility.formation_weight");
pub const REFERENCED_WEIGHT: ConfigKey<f64> =
    ConfigKey::new("memory.accessibility.use_weights.referenced");
pub const ACTED_ON_WEIGHT: ConfigKey<f64> =
    ConfigKey::new("memory.accessibility.use_weights.acted_on");
pub const RESULT_SUPPORTED_WEIGHT: ConfigKey<f64> =
    ConfigKey::new("memory.accessibility.use_weights.result_supported");
pub const RESULT_REFUTED_WEIGHT: ConfigKey<f64> =
    ConfigKey::new("memory.accessibility.use_weights.result_refuted");
pub const CORRECTED_WEIGHT: ConfigKey<f64> =
    ConfigKey::new("memory.accessibility.use_weights.corrected");
pub const PINNED_WEIGHT: ConfigKey<f64> = ConfigKey::new("memory.accessibility.use_weights.pinned");
pub const NORMAL_THRESHOLD: ConfigKey<f64> =
    ConfigKey::new("memory.accessibility.normal_threshold");
pub const DEEP_THRESHOLD: ConfigKey<f64> = ConfigKey::new("memory.accessibility.deep_threshold");

pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    let reference = nous_configuration::ReferenceProfile::parse(include_str!(
        "../../../../config/reference/memory-accessibility.json"
    ))?;
    let positive = |value: &f64| {
        if value.is_finite() && *value > 0.0 {
            Ok(())
        } else {
            Err(Error::Invalid(
                "accessibility value must be finite and positive".into(),
            ))
        }
    };
    let nonnegative = |value: &f64| {
        if value.is_finite() && *value >= 0.0 {
            Ok(())
        } else {
            Err(Error::Invalid(
                "accessibility weight must be finite and nonnegative".into(),
            ))
        }
    };
    let threshold = |value: &f64| {
        if value.is_finite() {
            Ok(())
        } else {
            Err(Error::Invalid(
                "accessibility threshold must be finite".into(),
            ))
        }
    };
    for (key, description, validator) in [
        (
            EPSILON,
            "Accessibility epsilon.",
            positive as fn(&f64) -> Result<()>,
        ),
        (
            TAU_DAYS,
            "Accessibility decay time constant in days.",
            positive,
        ),
        (DECAY, "Accessibility power decay.", positive),
        (
            FORMATION_WEIGHT,
            "Initial formation contribution.",
            nonnegative,
        ),
        (
            REFERENCED_WEIGHT,
            "Meaningful referenced-use weight.",
            nonnegative,
        ),
        (
            ACTED_ON_WEIGHT,
            "Meaningful acted-on-use weight.",
            nonnegative,
        ),
        (
            RESULT_SUPPORTED_WEIGHT,
            "Result-supported-use weight.",
            nonnegative,
        ),
        (
            RESULT_REFUTED_WEIGHT,
            "Result-refuted-use weight.",
            nonnegative,
        ),
        (CORRECTED_WEIGHT, "Corrected-use weight.", nonnegative),
        (PINNED_WEIGHT, "Pinned-use weight.", nonnegative),
        (
            NORMAL_THRESHOLD,
            "Normal accessibility threshold.",
            threshold,
        ),
        (DEEP_THRESHOLD, "Deep accessibility threshold.", threshold),
    ] {
        registry.register(
            key,
            "memory-service",
            description,
            reference.get(key)?,
            ConfigExposure::Developer,
            ConfigScopePolicy::SubjectOverrideAllowed,
            ConfigApplyMode::Live,
            ConfigSemanticEffect::Operational,
            validator,
        )?;
    }
    super::longitudinal_policy::register_longitudinal_configuration(registry)?;
    super::concept_maintenance::register_concept_configuration(registry)?;
    super::accretion::register_accretion_configuration(registry)?;
    for key in [EPSILON, TAU_DAYS, DECAY] {
        registry.describe(key.path(), |d| {
            d.json_schema["exclusiveMinimum"] = serde_json::json!(0);
        })?;
    }
    for key in [
        FORMATION_WEIGHT,
        REFERENCED_WEIGHT,
        ACTED_ON_WEIGHT,
        RESULT_SUPPORTED_WEIGHT,
        RESULT_REFUTED_WEIGHT,
        CORRECTED_WEIGHT,
        PINNED_WEIGHT,
    ] {
        registry.describe(key.path(), |d| {
            d.json_schema["minimum"] = serde_json::json!(0);
        })?;
    }
    registry.describe(TAU_DAYS.path(), |d| {
        d.unit = Some("days".into());
    })?;
    reference.describe(registry)?;
    Ok(())
}

pub fn resolve_accessibility_policy(snapshot: &ConfigSnapshot) -> Result<AccessibilityPolicy> {
    AccessibilityPolicy {
        epsilon: snapshot.get(EPSILON)?,
        tau_days: snapshot.get(TAU_DAYS)?,
        decay: snapshot.get(DECAY)?,
        formation_weight: snapshot.get(FORMATION_WEIGHT)?,
        referenced_weight: snapshot.get(REFERENCED_WEIGHT)?,
        acted_on_weight: snapshot.get(ACTED_ON_WEIGHT)?,
        result_supported_weight: snapshot.get(RESULT_SUPPORTED_WEIGHT)?,
        result_refuted_weight: snapshot.get(RESULT_REFUTED_WEIGHT)?,
        corrected_weight: snapshot.get(CORRECTED_WEIGHT)?,
        pinned_weight: snapshot.get(PINNED_WEIGHT)?,
        normal_threshold: snapshot.get(NORMAL_THRESHOLD)?,
        deep_threshold: snapshot.get(DEEP_THRESHOLD)?,
    }
    .validate()
}

pub fn eligible(level: AccessibilityLevel, effort: CognitiveEffort, exact: bool) -> bool {
    if exact {
        return true;
    }
    match level {
        AccessibilityLevel::Normal => true,
        AccessibilityLevel::Deep => {
            matches!(effort, CognitiveEffort::Deep | CognitiveEffort::Maximum)
        }
        AccessibilityLevel::Explicit => false,
    }
}

impl MemoryService {
    pub async fn set_accessibility(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        expected_object_epoch: i64,
        mode: AccessibilityMode,
    ) -> Result<MemoryView> {
        let digest = operation_digest(
            "set_accessibility",
            subject,
            &serde_json::json!({"memory_id":memory,"expected_object_epoch":expected_object_epoch,"mode":mode}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation_id, "set_accessibility", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return self.memory(subject, memory, None).await;
                }
                return Err(Error::Unavailable(
                    "accessibility operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let epoch:i64=sqlx::query_scalar("SELECT object_epoch FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE").bind(subject.0).bind(memory.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?.ok_or_else(||Error::NotFound("memory not found".into()))?;
        crate::service::state::fence_epoch(epoch, expected_object_epoch)?;
        sqlx::query("UPDATE memory_objects SET accessibility_mode=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND memory_id=$2").bind(subject.0).bind(memory.0).bind(format!("{mode:?}").to_lowercase()).execute(&mut **mutation.tx()).await.map_err(db)?;
        mutation
            .invalidate(ProjectionInvalidation {
                ..Default::default()
            })
            .await?;

        mutation
            .commit("memory", Some(&memory.0.to_string()), None, Some(epoch + 1))
            .await?;
        self.memory(subject, memory, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_values_match_the_frozen_profile() {
        let policy = AccessibilityPolicy::default();
        assert!((policy.activation(0.0, &[]) - 0.019802627).abs() < 1e-6);
        assert_eq!(
            policy.level_from_activation(policy.activation(180.0, &[])),
            AccessibilityLevel::Deep
        );
        assert_eq!(
            policy.level_from_activation(policy.activation(1000.0, &[])),
            AccessibilityLevel::Explicit
        );
    }

    #[test]
    fn meaningful_use_weights_and_mode_gates_are_distinct() {
        let policy = AccessibilityPolicy::default();
        let base = policy.activation(0.0, &[]);
        for kind in [
            "referenced",
            "acted_on",
            "result_supported",
            "result_refuted",
            "corrected",
            "pinned",
        ] {
            assert!(policy.activation(0.0, &[(kind.into(), 0.0)]) > base);
        }
        assert_eq!(policy.activation(0.0, &[("presented".into(), 0.0)]), base);
        assert!(eligible(
            AccessibilityLevel::Deep,
            CognitiveEffort::Deep,
            false
        ));
        assert!(!eligible(
            AccessibilityLevel::Deep,
            CognitiveEffort::Normal,
            false
        ));
        assert!(!eligible(
            AccessibilityLevel::Explicit,
            CognitiveEffort::Maximum,
            false
        ));
        assert!(eligible(
            AccessibilityLevel::Explicit,
            CognitiveEffort::Light,
            true
        ));
    }
}
