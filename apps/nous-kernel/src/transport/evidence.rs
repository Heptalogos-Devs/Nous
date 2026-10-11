// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::Result;
use nous_persistence::database_error as db;
use sqlx::Row;
use uuid::Uuid;
impl KernelService {
    pub(super) async fn get_occurrence(&self, input: p::ObjectRequest) -> Result<p::Occurrence> {
        let r = sqlx::query(
            "SELECT * FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$2",
        )
        .bind(id(&input.subject_id)?)
        .bind(id(&input.id)?)
        .fetch_one(self.0.store.pool())
        .await
        .map_err(db)?;
        let occurred_kind: String = r.try_get("occurred_time_kind").map_err(db)?;
        let occurred_start: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("occurred_time_start").map_err(db)?;
        let occurred_end: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("occurred_time_end").map_err(db)?;
        Ok(p::Occurrence {
            occurrence_id: input.id,
            subject_id: input.subject_id,
            artifact_id: r
                .try_get::<Option<Uuid>, _>("artifact_id")
                .map_err(db)?
                .map(|i| i.to_string()),
            source_class: r.try_get("source_class").map_err(db)?,
            external_object_ref: r.try_get("external_object_ref").map_err(db)?,
            occurred_time: Some(match occurred_kind.as_str() {
                "instant" => p::TemporalExtent {
                    value: Some(p::temporal_extent::Value::Instant(timestamp(
                        occurred_start.ok_or_else(|| {
                            Error::new(
                                nous_core::ErrorOwner::Observation,
                                nous_core::ErrorCode::InternalFailure,
                                "instant occurrence has no time",
                            )
                        })?,
                    ))),
                },
                "interval" => p::TemporalExtent {
                    value: Some(p::temporal_extent::Value::Interval(p::TimeInterval {
                        start: occurred_start.map(timestamp),
                        end: occurred_end.map(timestamp),
                    })),
                },
                _ => p::TemporalExtent { value: None },
            }),
            observed_at: Some(timestamp(r.try_get("observed_at").map_err(db)?)),
            conversation_ref: r.try_get("conversation_ref").map_err(db)?,
            actor_entity_ref: r.try_get("actor_entity_ref").map_err(db)?,
            context: to_object(r.try_get("context").map_err(db)?),
        })
    }
    pub(super) async fn get_source_region(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::SourceRegion> {
        let r =
            sqlx::query("SELECT * FROM source_regions WHERE subject_id=$1 AND source_region_id=$2")
                .bind(id(&input.subject_id)?)
                .bind(id(&input.id)?)
                .fetch_one(self.0.store.pool())
                .await
                .map_err(db)?;
        Ok(p::SourceRegion {
            source_region_id: input.id,
            artifact_id: r.try_get::<Uuid, _>("artifact_id").map_err(db)?.to_string(),
            coordinate_kind: r.try_get("coordinate_kind").map_err(db)?,
            coordinate: to_object(r.try_get("coordinate").map_err(db)?),
            coordinate_hash: r.try_get("coordinate_hash").map_err(db)?,
            parent_source_region_id: r
                .try_get::<Option<Uuid>, _>("parent_source_region_id")
                .map_err(db)?
                .map(|i| i.to_string()),
        })
    }
    pub(super) async fn get_derived_representation(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::DerivedRepresentation> {
        let r=sqlx::query("SELECT d.*,p.signature_hash,to_jsonb(p) AS producer FROM derived_representations d JOIN producer_signatures p ON p.producer_signature_id=d.producer_signature_id WHERE d.subject_id=$1 AND d.derived_representation_id=$2").bind(id(&input.subject_id)?).bind(id(&input.id)?).fetch_one(self.0.store.pool()).await.map_err(db)?;
        let representation_id = id(&input.id)?;
        let producer: serde_json::Value = r.try_get("producer").map_err(db)?;
        let field = |key: &str| {
            producer
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Observation,
                        nous_core::ErrorCode::InvalidRequest,
                        "Invalid request",
                    )
                })
        };
        Ok(p::DerivedRepresentation {
            representation_id: input.id,
            subject_id: input.subject_id,
            inputs: sqlx::query("SELECT * FROM derived_representation_inputs WHERE derived_representation_id=$1 ORDER BY ordinal").bind(representation_id).fetch_all(self.0.store.pool()).await.map_err(db)?.into_iter().map(|row| {
                let reference = if let Some(value)=row.try_get::<Option<Uuid>,_>("source_region_id").map_err(db)? { nous_core::CognitiveRef::SourceRegion(nous_core::SourceRegionId(value)) } else if let Some(value)=row.try_get::<Option<Uuid>,_>("input_representation_id").map_err(db)? { nous_core::CognitiveRef::DerivedRepresentation(nous_core::DerivedRepresentationId(value)) } else { nous_core::CognitiveRef::DerivedRegion(nous_core::DerivedRegionId(row.try_get("derived_region_id").map_err(db)?)) };
                Ok(p::DerivationInput { ordinal: row.try_get::<i32,_>("ordinal").map_err(db)? as u32, reference: Some(to_ref(reference)), role: row.try_get("role").map_err(db)? })
            }).collect::<Result<Vec<_>>>()?,
            producer: Some(p::ProducerSignature {
            model_role: producer.get("model_role").and_then(serde_json::Value::as_str).map(str::to_owned),
            model_profile: producer.get("model_profile").and_then(serde_json::Value::as_str).map(str::to_owned),
            execution_profile: producer.get("execution_profile").and_then(serde_json::Value::as_str).map(str::to_owned),
            inference_controls_digest: producer.get("inference_controls_digest").and_then(serde_json::Value::as_str).map(str::to_owned),
            role_policy_digest: producer.get("role_policy_digest").and_then(serde_json::Value::as_str).map(str::to_owned),
            prompt_id: producer.get("prompt_id").and_then(serde_json::Value::as_str).map(str::to_owned),
            prompt_digest: producer.get("prompt_digest").and_then(serde_json::Value::as_str).map(str::to_owned),
 signature_hash: field("signature_hash")?, provider_class: field("provider_class")?, operation: field("operation")?.replace('.', "_"), implementation: field("implementation")?, model_identity: producer.get("model_identity").and_then(serde_json::Value::as_str).map(str::to_owned), model_revision: producer.get("model_revision").and_then(serde_json::Value::as_str).map(str::to_owned), output_schema_digest: producer.get("output_schema_digest").and_then(serde_json::Value::as_str).map(str::to_owned), preprocessing_identity: field("preprocessing_identity")?, preprocessing_revision: field("preprocessing_revision")?, config_digest: field("config_digest")? }),
            quality: to_object(r.try_get("quality").map_err(db)?),
            supersedes: r.try_get::<Option<Uuid>,_>("supersedes").map_err(db)?.map(|id|id.to_string()),
            strategy: r.try_get("strategy").map_err(db)?,
            kind: r.try_get("representation_kind").map_err(db)?,
            producer_signature: r.try_get("signature_hash").map_err(db)?,
            revision: r.try_get("revision").map_err(db)?,
            text: r.try_get("payload_text").map_err(db)?,
            structured_payload: r.try_get::<Option<serde_json::Value>,_>("payload_json").map_err(db)?.and_then(to_object),
            artifact_id: r
                .try_get::<Option<Uuid>, _>("payload_artifact_id")
                .map_err(db)?
                .map(|i| i.to_string()),
            producer_signature_id: Some(
                r.try_get::<Uuid, _>("producer_signature_id")
                    .map_err(db)?
                    .to_string(),
            ),
        })
    }
}
