//! Complete material reads; transport consumes typed owner values.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;
use serde::de::DeserializeOwned;

fn decode<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|error| Error::Infrastructure(error.to_string()))
}

fn producer_value(mut value: serde_json::Value) -> Result<serde_json::Value> {
    let operation = value["operation"]
        .as_str()
        .ok_or_else(|| Error::Infrastructure("producer operation missing".into()))?;
    use CapabilityOperation::*;
    let operation = [
        TextEmbedding,
        TextRerank,
        TextInterpretation,
        MemoryFormationText,
        MemoryConsolidationText,
        ConceptMaintenanceText,
        EpisodeSegmentationText,
        JournalSynthesisText,
        ImageInterpretation,
        ImageEmbedding,
        SpeechTranscription,
        DocumentExtraction,
    ]
    .into_iter()
    .find(|candidate| candidate.as_str() == operation)
    .ok_or_else(|| Error::Infrastructure("unknown producer operation".into()))?;
    value["operation"] = serde_json::to_value(operation)
        .map_err(|error| Error::Infrastructure(error.to_string()))?;
    Ok(value)
}

const REPRESENTATION_READ: &str = "SELECT to_jsonb(d) || jsonb_build_object('producer',to_jsonb(p),'inputs',COALESCE((SELECT jsonb_agg(jsonb_build_object('ordinal',i.ordinal,'role',i.role,'reference',CASE WHEN i.source_region_id IS NOT NULL THEN jsonb_build_object('kind','source_region','id',i.source_region_id) WHEN i.input_representation_id IS NOT NULL THEN jsonb_build_object('kind','derived_representation','id',i.input_representation_id) ELSE jsonb_build_object('kind','derived_region','id',i.derived_region_id) END) ORDER BY i.ordinal) FROM derived_representation_inputs i WHERE i.derived_representation_id=d.derived_representation_id),'[]'::jsonb)) FROM derived_representations d JOIN producer_signatures p USING(producer_signature_id)";

fn decode_representation(mut value: serde_json::Value) -> Result<DerivedRepresentation> {
    value["producer"] = producer_value(value["producer"].take())?;
    decode(value)
}

impl MaterialService {
    pub async fn occurrence(
        &self,
        subject: SubjectId,
        occurrence: OccurrenceId,
    ) -> Result<ObservationOccurrence> {
        let row = sqlx::query(
            "SELECT to_jsonb(o) AS occurrence FROM observation_occurrences o WHERE subject_id=$1 AND occurrence_id=$2",
        )
        .bind(subject.0)
        .bind(occurrence.0)
        .fetch_optional(self.store.pool())
        .await
        .map_err(db)?
        .ok_or_else(|| Error::NotFound("Occurrence not found".into()))?;
        decode_occurrence(row)
    }

    pub async fn occurrences_for_artifact(
        &self,
        subject: SubjectId,
        artifact: ArtifactId,
        limit: u32,
    ) -> Result<(Vec<ObservationOccurrence>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("occurrence limit must be 1..200".into()));
        }
        self.store
            .validate_reference(subject, &CognitiveRef::Artifact(artifact))
            .await?;
        let mut rows = sqlx::query("SELECT to_jsonb(o) AS occurrence FROM observation_occurrences o WHERE subject_id=$1 AND artifact_id=$2 ORDER BY observed_at DESC,occurrence_id DESC LIMIT $3")
            .bind(subject.0).bind(artifact.0).bind(i64::from(limit)+1).fetch_all(self.store.pool()).await.map_err(db)?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        Ok((
            rows.into_iter()
                .map(decode_occurrence)
                .collect::<Result<_>>()?,
            more,
        ))
    }

    pub async fn source_region(
        &self,
        subject: SubjectId,
        region: SourceRegionId,
    ) -> Result<SourceRegion> {
        let value: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT to_jsonb(r) FROM source_regions r WHERE subject_id=$1 AND source_region_id=$2",
        )
        .bind(subject.0)
        .bind(region.0)
        .fetch_optional(self.store.pool())
        .await
        .map_err(db)?;
        decode(value.ok_or_else(|| Error::NotFound("SourceRegion not found".into()))?)
    }

    pub async fn derived_region(
        &self,
        subject: SubjectId,
        region: DerivedRegionId,
    ) -> Result<DerivedRegion> {
        let value: Option<serde_json::Value> = sqlx::query_scalar("SELECT to_jsonb(r) FROM derived_regions r WHERE subject_id=$1 AND derived_region_id=$2")
            .bind(subject.0).bind(region.0).fetch_optional(self.store.pool()).await.map_err(db)?;
        decode(value.ok_or_else(|| Error::NotFound("DerivedRegion not found".into()))?)
    }

    /// The composition boundary must establish that this signature is referenced by its Subject.
    pub async fn producer_signature(&self, producer: Uuid) -> Result<ProducerSignature> {
        let value: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT to_jsonb(p) FROM producer_signatures p WHERE producer_signature_id=$1",
        )
        .bind(producer)
        .fetch_optional(self.store.pool())
        .await
        .map_err(db)?;
        decode(producer_value(value.ok_or_else(|| {
            Error::NotFound("ProducerSignature not found".into())
        })?)?)
    }

    pub async fn derived_representation(
        &self,
        subject: SubjectId,
        representation: DerivedRepresentationId,
    ) -> Result<DerivedRepresentation> {
        let query = format!(
            "{REPRESENTATION_READ} WHERE d.subject_id=$1 AND d.derived_representation_id=$2"
        );
        let value = sqlx::query_scalar(sqlx::AssertSqlSafe(query.as_str()))
            .bind(subject.0)
            .bind(representation.0)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("DerivedRepresentation not found".into()))?;
        decode_representation(value)
    }

    pub async fn derived_representations(
        &self,
        subject: SubjectId,
        source: Option<Uuid>,
        artifact: Option<Uuid>,
        kind: Option<String>,
        limit: u32,
    ) -> Result<(Vec<DerivedRepresentation>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("representation limit must be 1..200".into()));
        }
        self.store.require_subject(subject).await?;
        let query = format!(
            "{REPRESENTATION_READ} WHERE d.subject_id=$1 AND ($2::uuid IS NULL OR EXISTS(SELECT 1 FROM representation_source_regions($1,d.derived_representation_id) roots WHERE roots.source_region_id=$2)) AND ($3::uuid IS NULL OR EXISTS(SELECT 1 FROM representation_source_regions($1,d.derived_representation_id) roots JOIN source_regions s USING(source_region_id) WHERE s.artifact_id=$3)) AND ($4::text IS NULL OR d.representation_kind=$4) AND NOT EXISTS(SELECT 1 FROM derived_representations newer WHERE newer.supersedes=d.derived_representation_id) ORDER BY d.derived_representation_id DESC LIMIT $5"
        );
        let mut values = sqlx::query_scalar(sqlx::AssertSqlSafe(query.as_str()))
            .bind(subject.0)
            .bind(source)
            .bind(artifact)
            .bind(kind)
            .bind(i64::from(limit) + 1)
            .fetch_all(self.store.pool())
            .await
            .map_err(db)?;
        let more = values.len() > limit as usize;
        values.truncate(limit as usize);
        Ok((
            values
                .into_iter()
                .map(decode_representation)
                .collect::<Result<_>>()?,
            more,
        ))
    }
}

fn decode_occurrence(row: sqlx::postgres::PgRow) -> Result<ObservationOccurrence> {
    let mut value: serde_json::Value = row.try_get("occurrence").map_err(db)?;
    let start: Option<DateTime<Utc>> = decode(value["occurred_time_start"].clone())?;
    let end: Option<DateTime<Utc>> = decode(value["occurred_time_end"].clone())?;
    let time = match value["occurred_time_kind"].as_str().unwrap_or("") {
        "instant" => TemporalExtent::Instant {
            at: start
                .ok_or_else(|| Error::Infrastructure("instant occurrence has no time".into()))?,
        },
        "interval" => TemporalExtent::Interval { start, end },
        "unknown" => TemporalExtent::Unknown,
        _ => return Err(Error::Infrastructure("invalid occurrence time kind".into())),
    };
    value["occurred_time"] =
        serde_json::to_value(time).map_err(|error| Error::Infrastructure(error.to_string()))?;
    decode(value)
}

impl MaterialService {
    pub async fn maintenance_sources(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
    ) -> Result<Vec<(ObservationOccurrence, CognitiveRef)>> {
        let ids: Vec<_> = occurrences.iter().map(|id| id.0).collect();
        sqlx::query("SELECT to_jsonb(o) AS occurrence,d.derived_representation_id FROM observation_occurrences o LEFT JOIN LATERAL (SELECT r.derived_representation_id FROM coverage_needs c JOIN source_regions s USING(source_region_id) JOIN derived_representations r ON r.derived_representation_id=c.current_representation_id WHERE c.subject_id=$1 AND s.artifact_id=o.artifact_id AND c.state='ready' AND r.payload_text IS NOT NULL ORDER BY r.created_at DESC,r.derived_representation_id LIMIT 1) d ON true WHERE o.subject_id=$1 AND o.occurrence_id=ANY($2::uuid[])")
            .bind(subject.0).bind(ids).fetch_all(self.store.pool()).await.map_err(db)?.into_iter().map(|row| {
                let derived = row.try_get::<Option<Uuid>,_>("derived_representation_id").map_err(db)?;
                let occurrence = decode_occurrence(row)?;
                let reference = derived.map(|id| CognitiveRef::DerivedRepresentation(DerivedRepresentationId(id))).unwrap_or(CognitiveRef::Occurrence(occurrence.occurrence_id));
                Ok((occurrence,reference))
            }).collect()
    }
}
