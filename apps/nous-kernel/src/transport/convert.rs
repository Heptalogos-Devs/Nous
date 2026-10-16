// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use chrono::{DateTime, Utc};
use nous_core::{CognitiveRef, Result, TemporalExtent};
use nous_memory::{CognitionDependency, EvidenceLocator, EvidenceRef, RevisionBasis};
use prost_types::{Struct, Timestamp, Value, value::Kind};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use uuid::Uuid;

pub fn id(value: &str) -> Result<Uuid> {
    value
        .parse()
        .map_err(|_| Error::Invalid("invalid UUID".into()))
}
pub fn required<T>(value: Option<T>, name: &str) -> Result<T> {
    value.ok_or_else(|| Error::Invalid(format!("{name} is required")))
}
pub fn enum_value<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(value.into()))
        .map_err(|_| Error::Invalid(format!("invalid enum value: {value}")))
}
pub fn enum_name<T: Serialize>(value: T) -> String {
    serde_json::to_value(value).map_or_else(
        |_| "invalid_enum".into(),
        |value| {
            value
                .as_str()
                .map_or_else(|| "invalid_enum".into(), str::to_owned)
        },
    )
}
pub fn time(value: Option<Timestamp>) -> Result<Option<DateTime<Utc>>> {
    value
        .map(|value| {
            if !(0..1_000_000_000).contains(&value.nanos) {
                return Err(Error::Invalid("invalid timestamp nanos".into()));
            }
            DateTime::from_timestamp(value.seconds, value.nanos as u32)
                .ok_or_else(|| Error::Invalid("invalid timestamp".into()))
        })
        .transpose()
}
pub fn timestamp(value: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: value.timestamp(),
        nanos: value.timestamp_subsec_nanos() as i32,
    }
}

pub fn temporal(value: Option<p::TemporalExtent>) -> Result<TemporalExtent> {
    let Some(value) = value else {
        return Ok(TemporalExtent::Unknown);
    };
    match value.value {
        Some(p::temporal_extent::Value::Instant(value)) => Ok(TemporalExtent::Instant {
            at: time(Some(value))?.ok_or_else(|| Error::Invalid("invalid instant".into()))?,
        }),
        Some(p::temporal_extent::Value::Interval(value)) => Ok(TemporalExtent::Interval {
            start: time(value.start)?,
            end: time(value.end)?,
        }),
        None => Ok(TemporalExtent::Unknown),
    }
}

pub fn temporal_proto(value: &TemporalExtent) -> p::TemporalExtent {
    p::TemporalExtent {
        value: match value {
            TemporalExtent::Unknown => None,
            TemporalExtent::Instant { at } => {
                Some(p::temporal_extent::Value::Instant(timestamp(*at)))
            }
            TemporalExtent::Interval { start, end } => {
                Some(p::temporal_extent::Value::Interval(p::TimeInterval {
                    start: start.map(timestamp),
                    end: end.map(timestamp),
                }))
            }
        },
    }
}

pub fn material_source(value: p::EvidenceRef) -> Result<EvidenceRef> {
    match basis(p::RevisionBasis {
        basis: Some(p::revision_basis::Basis::Evidence(value)),
    })? {
        RevisionBasis::Evidence(value) => Ok(value),
        _ => unreachable!("evidence wrapper"),
    }
}

pub fn basis(value: p::RevisionBasis) -> Result<RevisionBasis> {
    match value
        .basis
        .ok_or_else(|| Error::Invalid("revision support is empty".into()))?
    {
        p::revision_basis::Basis::Evidence(value) => {
            let locator = match value
                .locator
                .ok_or_else(|| Error::Invalid("evidence locator is empty".into()))?
            {
                p::evidence_ref::Locator::WholeOccurrence(_) => EvidenceLocator::WholeOccurrence,
                p::evidence_ref::Locator::SourceRegionId(value) => {
                    EvidenceLocator::SourceRegion(nous_core::SourceRegionId(id(&value)?))
                }
                p::evidence_ref::Locator::DerivedRepresentationId(value) => {
                    EvidenceLocator::DerivedRepresentation(nous_core::DerivedRepresentationId(id(
                        &value,
                    )?))
                }
                p::evidence_ref::Locator::DerivedRegionId(value) => {
                    EvidenceLocator::DerivedRegion(nous_core::DerivedRegionId(id(&value)?))
                }
            };
            Ok(RevisionBasis::Evidence(EvidenceRef {
                epistemic_relation: value
                    .epistemic_relation
                    .as_deref()
                    .map(enum_value)
                    .transpose()?,
                occurrence_id: nous_core::OccurrenceId(id(&value.occurrence_id)?),
                locator,
                basis_role: enum_value(&value.basis_role)?,
            }))
        }
        p::revision_basis::Basis::CognitionDependency(value) => {
            Ok(RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: value
                    .epistemic_relation
                    .as_deref()
                    .map(enum_value)
                    .transpose()?,
                target_revision: from_ref(required(value.target_revision, "target_revision")?)?,
                basis_role: enum_value(&value.basis_role)?,
            }))
        }
        p::revision_basis::Basis::Seed(value) => {
            let reference = from_ref(required(value.seed_version, "seed_version")?)?;
            match reference {
                CognitiveRef::CognitiveSeedVersion(id) => Ok(RevisionBasis::Seed(
                    nous_core::SeedBasisRef::new(id, value.semantic_path)?,
                )),
                _ => Err(Error::Invalid(
                    "seed support must target a Cognitive Seed version".into(),
                )),
            }
        }
    }
}

pub fn basis_proto(value: RevisionBasis) -> p::RevisionBasis {
    let basis = match value {
        RevisionBasis::Evidence(value) => {
            let locator = match value.locator {
                EvidenceLocator::WholeOccurrence => p::evidence_ref::Locator::WholeOccurrence(true),
                EvidenceLocator::SourceRegion(id) => {
                    p::evidence_ref::Locator::SourceRegionId(id.0.to_string())
                }
                EvidenceLocator::DerivedRepresentation(id) => {
                    p::evidence_ref::Locator::DerivedRepresentationId(id.0.to_string())
                }
                EvidenceLocator::DerivedRegion(id) => {
                    p::evidence_ref::Locator::DerivedRegionId(id.0.to_string())
                }
            };
            p::revision_basis::Basis::Evidence(p::EvidenceRef {
                epistemic_relation: value.epistemic_relation.map(enum_name),
                occurrence_id: value.occurrence_id.0.to_string(),
                locator: Some(locator),
                basis_role: enum_name(value.basis_role),
            })
        }
        RevisionBasis::CognitionDependency(value) => {
            p::revision_basis::Basis::CognitionDependency(p::CognitionDependency {
                epistemic_relation: value.epistemic_relation.map(enum_name),
                target_revision: Some(to_ref(value.target_revision)),
                basis_role: enum_name(value.basis_role),
            })
        }
        RevisionBasis::Seed(value) => p::revision_basis::Basis::Seed(p::SeedBasisRef {
            seed_version: Some(to_ref(CognitiveRef::CognitiveSeedVersion(
                value.seed_version_id,
            ))),
            semantic_path: value.semantic_path,
        }),
    };
    p::RevisionBasis { basis: Some(basis) }
}
pub fn from_ref(value: p::CognitiveRef) -> Result<CognitiveRef> {
    nous_core::parse_reference(&value.kind, &value.value)
}
pub fn from_producer(p: p::ProducerSignature) -> Result<nous_core::ProducerSignature> {
    Ok(nous_core::ProducerSignature {
        model_role: p.model_role,
        model_profile: p.model_profile,
        execution_profile: p.execution_profile,
        inference_controls_digest: p.inference_controls_digest,
        role_policy_digest: p.role_policy_digest,
        prompt_id: p.prompt_id,
        prompt_digest: p.prompt_digest,

        signature_hash: String::new(),
        provider_class: p.provider_class,
        operation: enum_value(&p.operation)?,
        implementation: p.implementation,
        model_identity: p.model_identity,
        model_revision: p.model_revision,
        output_schema_digest: p.output_schema_digest,
        preprocessing_identity: p.preprocessing_identity,
        preprocessing_revision: p.preprocessing_revision,
        config_digest: p.config_digest,
    })
}
pub fn to_ref(value: CognitiveRef) -> p::CognitiveRef {
    let (kind, value) = nous_core::reference_parts(&value);
    p::CognitiveRef { kind, value }
}
const MAX_JSON_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

pub fn json(value: Value) -> serde_json::Value {
    match value.kind {
        None | Some(Kind::NullValue(_)) => serde_json::Value::Null,
        Some(Kind::BoolValue(value)) => value.into(),
        Some(Kind::NumberValue(value))
            if value.fract() == 0.0 && value.abs() <= MAX_JSON_SAFE_INTEGER =>
        {
            serde_json::json!(value as i64)
        }
        Some(Kind::NumberValue(value)) => serde_json::json!(value),
        Some(Kind::StringValue(value)) => value.into(),
        Some(Kind::ListValue(value)) => {
            serde_json::Value::Array(value.values.into_iter().map(json).collect())
        }
        Some(Kind::StructValue(value)) => object(Some(value)),
    }
}
pub fn object(value: Option<Struct>) -> serde_json::Value {
    serde_json::Value::Object(
        value
            .unwrap_or_default()
            .fields
            .into_iter()
            .map(|(key, value)| (key, json(value)))
            .collect(),
    )
}
pub fn value(json: serde_json::Value) -> Value {
    Value {
        kind: Some(match json {
            serde_json::Value::Null => Kind::NullValue(0),
            serde_json::Value::Bool(v) => Kind::BoolValue(v),
            serde_json::Value::Number(v) => v
                .as_f64()
                .map_or_else(|| Kind::StringValue(v.to_string()), Kind::NumberValue),
            serde_json::Value::String(v) => Kind::StringValue(v),
            serde_json::Value::Array(v) => Kind::ListValue(prost_types::ListValue {
                values: v.into_iter().map(value).collect(),
            }),
            serde_json::Value::Object(v) => Kind::StructValue(Struct {
                fields: v.into_iter().map(|(k, v)| (k, value(v))).collect(),
            }),
        }),
    }
}
pub fn to_object(json: serde_json::Value) -> Option<Struct> {
    match value(json).kind {
        Some(Kind::StructValue(v)) => Some(v),
        _ => Some(Struct {
            fields: BTreeMap::new(),
        }),
    }
}
pub fn page(page: Option<p::Page>, scope: &str) -> Result<(i64, Option<Uuid>)> {
    let (limit, last) = page_cursor(page, scope)?;
    Ok((limit, last.as_deref().map(id).transpose()?))
}
pub fn page_cursor(page: Option<p::Page>, scope: &str) -> Result<(i64, Option<String>)> {
    let page = page.unwrap_or_default();
    if page.page_size > 200 {
        return Err(Error::Invalid("page_size exceeds 200".into()));
    }
    let last = if page.page_token.is_empty() {
        None
    } else {
        let (digest, last) = page
            .page_token
            .split_once('.')
            .ok_or_else(|| Error::Invalid("invalid page token".into()))?;
        if digest != blake3::hash(scope.as_bytes()).to_hex().as_str() {
            return Err(Error::Invalid(
                "page token belongs to another request".into(),
            ));
        }
        Some(last.to_owned())
    };
    Ok((
        i64::from(if page.page_size == 0 {
            50
        } else {
            page.page_size
        }),
        last,
    ))
}
pub fn next_token(scope: &str, id: impl std::fmt::Display) -> String {
    format!("{}.{}", blake3::hash(scope.as_bytes()).to_hex(), id)
}

pub fn domain_temporal(
    value: Option<p::DomainTemporalExtent>,
) -> Result<nous_core::DomainTemporalExtent> {
    let Some(value) = value else {
        return Ok(nous_core::DomainTemporalExtent::default());
    };
    let result = nous_core::DomainTemporalExtent {
        time_domain_ref: value.time_domain_ref,
        extent: temporal(value.extent)?,
    };
    result.validate()?;
    Ok(result)
}
pub fn domain_temporal_proto(domain: &str, value: &TemporalExtent) -> p::DomainTemporalExtent {
    p::DomainTemporalExtent {
        time_domain_ref: domain.into(),
        extent: Some(temporal_proto(value)),
    }
}
pub fn domain_stamp_proto(value: &nous_core::DomainStamp) -> p::DomainStamp {
    p::DomainStamp {
        time_domain_ref: value.time_domain_ref.clone(),
        at: Some(timestamp(value.at)),
        recorded_at: Some(timestamp(value.recorded_at)),
    }
}

pub fn domain_stamp(value: p::DomainStamp) -> Result<nous_core::DomainStamp> {
    if value.time_domain_ref.trim().is_empty() || value.time_domain_ref.len() > 256 {
        return Err(Error::Invalid("invalid time domain".into()));
    }
    Ok(nous_core::DomainStamp {
        time_domain_ref: value.time_domain_ref,
        at: required(time(value.at)?, "semantic at")?,
        recorded_at: required(time(value.recorded_at)?, "real recorded_at")?,
    })
}

pub fn formation_proto(
    domain: &str,
    at: DateTime<Utc>,
    recorded_at: DateTime<Utc>,
) -> p::DomainStamp {
    p::DomainStamp {
        time_domain_ref: domain.into(),
        at: Some(timestamp(at)),
        recorded_at: Some(timestamp(recorded_at)),
    }
}

pub fn temporal_evidence_proto(value: &nous_core::TemporalEvidence) -> p::TemporalEvidence {
    p::TemporalEvidence {
        sources: value
            .sources
            .iter()
            .map(|source| p::SourceTemporalEvidence {
                occurrence_id: source.occurrence_id.0.to_string(),
                occurred: Some(domain_temporal_proto(
                    &source.occurred.time_domain_ref,
                    &source.occurred.extent,
                )),
                observed: Some(domain_stamp_proto(&source.observed)),
                observed_at: Some(timestamp(source.observed_at)),
                recorded_at: Some(timestamp(source.recorded_at)),
            })
            .collect(),
    }
}

pub fn temporal_metadata_proto(
    value: &nous_core::FreshnessDescriptor,
) -> p::DomainTemporalEvidence {
    p::DomainTemporalEvidence {
        temporal_evidence: Some(temporal_evidence_proto(&value.temporal_evidence)),
        valid: Some(domain_temporal_proto(
            &value.valid_time.time_domain_ref,
            &value.valid_time.extent,
        )),
        formed: value.formed_at.as_ref().map(domain_stamp_proto),
        recorded_at: value.recorded_at.map(timestamp),
    }
}
