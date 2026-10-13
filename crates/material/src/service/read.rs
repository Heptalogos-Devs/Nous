//! Complete material reads; transport consumes typed owner values.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub(super) struct Payload {
    pub(super) text: Option<String>,
    pub(super) hash: Option<String>,
    pub(super) media_type: String,
    pub(super) length: u64,
    pub(super) selected: Option<ByteRange>,
    pub(super) provenance: Vec<CognitiveRef>,
    pub(super) producer: Option<serde_json::Value>,
}

/// Request-scoped resolved metadata. Payload bytes are read only when consumed.
#[derive(Clone)]
pub struct MaterialReadInput {
    pub(super) reference: CognitiveRef,
    pub(super) payload: Result<Payload>,
    pub(super) source_artifacts: Vec<Uuid>,
    pub(super) view_digest: Option<String>,
}

fn decode<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|error| Error::Infrastructure(error.to_string()))
}

const REPRESENTATION_READ: &str = "SELECT to_jsonb(d) || jsonb_build_object('producer',to_jsonb(p),'inputs',COALESCE((SELECT jsonb_agg(jsonb_build_object('ordinal',i.ordinal,'role',i.role,'reference',CASE WHEN i.source_region_id IS NOT NULL THEN jsonb_build_object('kind','source_region','id',i.source_region_id) WHEN i.input_representation_id IS NOT NULL THEN jsonb_build_object('kind','derived_representation','id',i.input_representation_id) ELSE jsonb_build_object('kind','derived_region','id',i.derived_region_id) END) ORDER BY i.ordinal) FROM derived_representation_inputs i WHERE i.derived_representation_id=d.derived_representation_id),'[]'::jsonb)) FROM derived_representations d JOIN producer_signatures p USING(producer_signature_id)";

pub(super) fn material_reference(reference: &CognitiveRef) -> Result<(&'static str, Uuid)> {
    match reference {
        CognitiveRef::Occurrence(_)
        | CognitiveRef::Artifact(_)
        | CognitiveRef::SourceRegion(_)
        | CognitiveRef::DerivedRepresentation(_)
        | CognitiveRef::DerivedRegion(_) => Ok((
            reference.kind(),
            reference.uuid().expect("Material references are UUIDs"),
        )),
        _ => Err(Error::Invalid("requires a Material reference".into())),
    }
}

impl MaterialService {
    pub(super) async fn lineage_nodes_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<Vec<MaterialLineageNode>> {
        let (kinds, ids): (Vec<_>, Vec<_>) = references
            .iter()
            .map(material_reference)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unzip();
        let query = format!(
            r#"
WITH requested AS (SELECT * FROM unnest($2::text[],$3::uuid[]) r(kind,id))
SELECT r.kind, CASE r.kind
 WHEN 'occurrence' THEN to_jsonb(o) WHEN 'artifact' THEN to_jsonb(a)
 WHEN 'source_region' THEN to_jsonb(s) WHEN 'derived_region' THEN to_jsonb(g)
 WHEN 'derived_representation' THEN d.body END AS node
FROM requested r
LEFT JOIN observation_occurrences o ON r.kind='occurrence' AND o.subject_id=$1 AND o.occurrence_id=r.id
LEFT JOIN artifacts a ON r.kind='artifact' AND a.subject_id=$1 AND a.artifact_id=r.id
LEFT JOIN source_regions s ON r.kind='source_region' AND s.subject_id=$1 AND s.source_region_id=r.id
LEFT JOIN derived_regions g ON r.kind='derived_region' AND g.subject_id=$1 AND g.derived_region_id=r.id
LEFT JOIN LATERAL ({REPRESENTATION_READ} WHERE r.kind='derived_representation' AND d.subject_id=$1 AND d.derived_representation_id=r.id) d(body) ON true
"#
        );
        sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
            .bind(subject.0)
            .bind(kinds)
            .bind(ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?
            .into_iter()
            .map(|row| {
                let value = row
                    .try_get::<Option<serde_json::Value>, _>("node")
                    .map_err(db)?
                    .ok_or_else(|| {
                        Error::NotFound("lineage contains a missing Material reference".into())
                    })?;
                Ok(
                    match row.try_get::<String, _>("kind").map_err(db)?.as_str() {
                        "occurrence" => MaterialLineageNode::Occurrence(occurrence_value(value)?),
                        "artifact" => MaterialLineageNode::Artifact(decode(value)?),
                        "source_region" => MaterialLineageNode::SourceRegion(decode(value)?),
                        "derived_region" => MaterialLineageNode::DerivedRegion(decode(value)?),
                        "derived_representation" => {
                            MaterialLineageNode::Representation(Box::new(decode(value)?))
                        }
                        _ => unreachable!("validated Material reference"),
                    },
                )
            })
            .collect()
    }

    /// Admit immutable source identities into the caller's mutation transaction in one batch.
    pub async fn lock_occurrences_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
    ) -> Result<std::collections::HashMap<OccurrenceId, DateTime<Utc>>> {
        let ids = occurrences
            .iter()
            .map(|id| id.0)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let rows = sqlx::query("SELECT occurrence_id,observed_at FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=ANY($2::uuid[]) ORDER BY occurrence_id FOR SHARE")
            .bind(subject.0).bind(&ids).fetch_all(&mut **tx).await.map_err(db)?;
        if rows.len() != ids.len() {
            return Err(Error::Invalid(
                "evidence occurrence is outside Subject".into(),
            ));
        }
        rows.into_iter()
            .map(|row| {
                Ok((
                    OccurrenceId(row.try_get("occurrence_id").map_err(db)?),
                    row.try_get("observed_at").map_err(db)?,
                ))
            })
            .collect()
    }

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

    /// Exact occurrence metadata in the same input order-independent bounded read surface.
    pub async fn occurrences(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<std::collections::HashMap<OccurrenceId, ObservationOccurrence>> {
        if occurrences.len() > 256 {
            return Err(Error::Invalid(
                "occurrence batch exceeds 256 references".into(),
            ));
        }
        let mut tx = self.store.read_view().await?;
        let result = self
            .occurrences_in(&mut tx, subject, occurrences, view)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }

    /// Source facts can participate in the caller's captured read without a second snapshot.
    pub async fn occurrences_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<std::collections::HashMap<OccurrenceId, ObservationOccurrence>> {
        for occurrence in occurrences {
            self.require_lineage_view(subject, &CognitiveRef::Occurrence(*occurrence), view)?;
        }
        let ids: Vec<_> = occurrences.iter().map(|id| id.0).collect();
        let rows = sqlx::query("SELECT to_jsonb(o) AS occurrence FROM observation_occurrences o WHERE subject_id=$1 AND occurrence_id=ANY($2::uuid[]) AND ($3::timestamptz IS NULL OR created_at<=$3)")
            .bind(subject.0).bind(ids).bind(view.map(|view|view.as_of)).fetch_all(&mut **tx).await.map_err(db)?;
        let result = rows
            .into_iter()
            .map(|row| {
                let occurrence = decode_occurrence(row)?;
                Ok((occurrence.occurrence_id, occurrence))
            })
            .collect::<Result<std::collections::HashMap<_, _>>>()?;
        if occurrences.iter().any(|id| !result.contains_key(id)) {
            return Err(Error::NotFound(
                "evidence occurrence is outside Subject/view".into(),
            ));
        }
        Ok(result)
    }

    pub async fn occurrence_observed_span(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
    ) -> Result<i64> {
        sqlx::query_scalar("SELECT COALESCE(extract(epoch FROM max(observed_at)-min(observed_at))::bigint,0) FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=ANY($2::uuid[])")
            .bind(subject.0).bind(occurrences.iter().map(|id| id.0).collect::<Vec<_>>()).fetch_one(self.store.pool()).await.map_err(db)
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

    /// IDs have already been authorized by the owning Subject operation.
    pub async fn producer_signatures(&self, producers: &[Uuid]) -> Result<Vec<ProducerSignature>> {
        if producers.len() > 2048 {
            return Err(Error::Invalid("producer batch exceeds bound".into()));
        }
        let rows: Vec<serde_json::Value> = sqlx::query_scalar("SELECT to_jsonb(p) FROM producer_signatures p WHERE producer_signature_id=ANY($1) ORDER BY producer_signature_id")
            .bind(producers).fetch_all(self.store.pool()).await.map_err(db)?;
        if rows.len() != producers.len() {
            return Err(Error::NotFound("ProducerSignature not found".into()));
        }
        rows.into_iter().map(decode).collect()
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
        decode(value.ok_or_else(|| Error::NotFound("ProducerSignature not found".into()))?)
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
        decode(value)
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
        Ok((values.into_iter().map(decode).collect::<Result<_>>()?, more))
    }
}

fn decode_occurrence(row: sqlx::postgres::PgRow) -> Result<ObservationOccurrence> {
    occurrence_value(row.try_get("occurrence").map_err(db)?)
}

fn occurrence_value(mut value: serde_json::Value) -> Result<ObservationOccurrence> {
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
    pub async fn maintenance_inputs(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
    ) -> Result<Vec<(ObservationOccurrence, MaterialReadInput)>> {
        self.store.require_subject(subject).await?;
        let mut tx = self.store.read_view().await?;
        let ids: Vec<_> = occurrences.iter().map(|id| id.0).collect();
        let sources = sqlx::query("SELECT to_jsonb(o) AS occurrence,d.derived_representation_id FROM observation_occurrences o LEFT JOIN LATERAL (SELECT r.derived_representation_id FROM coverage_needs c JOIN source_regions s USING(source_region_id) JOIN derived_representations r ON r.derived_representation_id=c.current_representation_id WHERE c.subject_id=$1 AND s.artifact_id=o.artifact_id AND c.state='ready' AND r.payload_text IS NOT NULL ORDER BY r.created_at DESC,r.derived_representation_id LIMIT 1) d ON true WHERE o.subject_id=$1 AND o.occurrence_id=ANY($2::uuid[])")
            .bind(subject.0).bind(ids).fetch_all(&mut *tx).await.map_err(db)?.into_iter().map(|row| {
                let derived = row.try_get::<Option<Uuid>,_>("derived_representation_id").map_err(db)?;
                let occurrence = decode_occurrence(row)?;
                let reference = derived.map(|id| CognitiveRef::DerivedRepresentation(DerivedRepresentationId(id))).unwrap_or(CognitiveRef::Occurrence(occurrence.occurrence_id));
                Ok((occurrence,reference))
            }).collect::<Result<Vec<_>>>()?;
        let references = sources
            .iter()
            .map(|(_, reference)| reference.clone())
            .collect::<Vec<_>>();
        let inputs = self
            .read_inputs_in(&mut tx, subject, &references, None, true)
            .await?;
        tx.commit().await.map_err(db)?;
        // Different observations may select the same representation; metadata remains request-scoped.
        sources
            .into_iter()
            .map(|(occurrence, reference)| {
                let input = inputs
                    .get(&reference)
                    .expect("selected Material input")
                    .clone();
                Ok((occurrence, input))
            })
            .collect()
    }
}

impl MaterialService {
    /// Resolve exact Material payload, producer and provenance metadata without reading object bytes.
    pub async fn read_inputs(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<std::collections::HashMap<CognitiveRef, MaterialReadInput>> {
        self.store.require_subject(subject).await?;
        let mut tx = self.store.read_view().await?;
        let result = self
            .read_inputs_in(&mut tx, subject, references, view, true)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }

    pub(super) async fn read_inputs_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
        read_text: bool,
    ) -> Result<std::collections::HashMap<CognitiveRef, MaterialReadInput>> {
        if references.len() > 256 {
            return Err(Error::Invalid(
                "Material text lookup supports at most 256 references".into(),
            ));
        }
        if view.is_some_and(|view| view.subject != subject) {
            return Err(Error::Invalid("Material view subject mismatch".into()));
        }
        if references.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut resolved = Vec::new();
        let mut kinds = Vec::new();
        let mut ids = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut result = std::collections::HashMap::new();
        for reference in references {
            if !seen.insert(reference) {
                continue;
            }
            if let Err(error) = self.require_lineage_view(subject, reference, view) {
                result.insert(
                    reference.clone(),
                    MaterialReadInput {
                        reference: reference.clone(),
                        payload: Err(error),
                        source_artifacts: vec![],
                        view_digest: view.map(|view| view.snapshot_digest.clone()),
                    },
                );
                continue;
            }
            let Ok((kind, id)) = material_reference(reference) else {
                let error = if !self
                    .store
                    .reference_in_subject_tx(tx, subject, reference)
                    .await?
                {
                    Error::NotFound(format!("{reference}: cognitive reference not found"))
                } else {
                    Error::NotFound(format!("{reference}: no Material payload"))
                };
                result.insert(
                    reference.clone(),
                    MaterialReadInput {
                        reference: reference.clone(),
                        payload: Err(error),
                        source_artifacts: vec![],
                        view_digest: view.map(|view| view.snapshot_digest.clone()),
                    },
                );
                continue;
            };
            resolved.push(reference);
            kinds.push(kind);
            ids.push(id);
        }
        if resolved.is_empty() {
            return Ok(result);
        }
        let rows = sqlx::query(r#"
WITH requested AS (SELECT kind,id,ordinality FROM unnest($2::text[],$3::uuid[]) WITH ORDINALITY AS r(kind,id,ordinality)),
resolved AS (
 SELECT r.*,o.occurrence_id IS NOT NULL OR a.artifact_id IS NOT NULL OR sr.source_region_id IS NOT NULL OR d.derived_representation_id IS NOT NULL OR dr.derived_region_id IS NOT NULL AS present,
 coalesce(o.artifact_id,a.artifact_id,sr.artifact_id) AS artifact_id,
 coalesce(d.derived_representation_id,dr.derived_representation_id) AS derived_id,
 coalesce(sr.coordinate_kind,dr.coordinate_kind) AS coordinate_kind,coalesce(sr.coordinate,dr.coordinate) AS coordinate
 FROM requested r
 LEFT JOIN observation_occurrences o ON r.kind='occurrence' AND o.subject_id=$1 AND o.occurrence_id=r.id
 LEFT JOIN artifacts a ON r.kind='artifact' AND a.subject_id=$1 AND a.artifact_id=r.id
 LEFT JOIN source_regions sr ON r.kind='source_region' AND sr.subject_id=$1 AND sr.source_region_id=r.id
 LEFT JOIN derived_representations d ON r.kind='derived_representation' AND d.subject_id=$1 AND d.derived_representation_id=r.id
 LEFT JOIN derived_regions dr ON r.kind='derived_region' AND dr.subject_id=$1 AND dr.derived_region_id=r.id
)
SELECT r.ordinality,r.present,r.artifact_id,r.derived_id,r.coordinate_kind,r.coordinate,
 CASE WHEN $4 THEN d.payload_text END AS payload_text,octet_length(d.payload_text) AS text_length,
 a.artifact_id AS payload_artifact_id,to_jsonb(p) AS producer,
 a.content_hash,a.media_type,a.byte_length,
 COALESCE(roots.regions,'{}'::uuid[]) AS source_roots,
 CASE WHEN r.derived_id IS NULL THEN array_remove(ARRAY[r.artifact_id],NULL) ELSE COALESCE(roots.artifacts,'{}'::uuid[]) END AS source_artifacts
FROM resolved r
LEFT JOIN derived_representations d ON d.subject_id=$1 AND d.derived_representation_id=r.derived_id
LEFT JOIN artifacts a ON a.subject_id=$1 AND a.artifact_id=coalesce(r.artifact_id,d.payload_artifact_id)
LEFT JOIN producer_signatures p ON p.producer_signature_id=d.producer_signature_id
LEFT JOIN LATERAL (SELECT array_agg(DISTINCT sr.source_region_id) AS regions,array_agg(DISTINCT sr.artifact_id) AS artifacts FROM representation_source_regions($1,r.derived_id) roots JOIN source_regions sr USING(source_region_id)) roots ON r.derived_id IS NOT NULL
"#).bind(subject.0).bind(kinds).bind(ids).bind(read_text).fetch_all(&mut **tx).await.map_err(db)?;
        for row in rows {
            let index: i64 = row.try_get("ordinality").map_err(db)?;
            let reference = resolved[index as usize - 1];
            let present: bool = row.try_get("present").map_err(db)?;
            let derived: Option<Uuid> = row.try_get("derived_id").map_err(db)?;
            let roots: Vec<Uuid> = row.try_get("source_roots").map_err(db)?;
            let source_artifacts: Vec<Uuid> = row.try_get("source_artifacts").map_err(db)?;
            let payload = if !present {
                Err(Error::NotFound(format!(
                    "{reference}: cognitive reference not found"
                )))
            } else if derived.is_some() && roots.is_empty() {
                Err(Error::Invalid(
                    "derived representation has no source roots".into(),
                ))
            } else {
                read_payload(&row, reference, derived, roots)
            };
            result.insert(
                reference.clone(),
                MaterialReadInput {
                    reference: reference.clone(),
                    payload,
                    source_artifacts,
                    view_digest: view.map(|view| view.snapshot_digest.clone()),
                },
            );
        }
        Ok(result)
    }
}

fn read_payload(
    row: &sqlx::postgres::PgRow,
    reference: &CognitiveRef,
    derived: Option<Uuid>,
    roots: Vec<Uuid>,
) -> Result<Payload> {
    let text: Option<String> = row.try_get("payload_text").map_err(db)?;
    let text_length: Option<i32> = row.try_get("text_length").map_err(db)?;
    let hash: Option<String> = row.try_get("content_hash").map_err(db)?;
    if text_length.is_none() && hash.is_none() {
        return Err(Error::Unavailable(
            "Material reference has no locally available byte/text payload".into(),
        ));
    }
    let (media_type, length) = if let Some(length) = text_length {
        ("text/plain; charset=utf-8".to_owned(), length as u64)
    } else {
        (
            row.try_get::<String, _>("media_type").map_err(db)?,
            row.try_get::<i64, _>("byte_length").map_err(db)? as u64,
        )
    };
    let coordinate: Option<serde_json::Value> = row.try_get("coordinate").map_err(db)?;
    let selected = if let Some(coordinate) = &coordinate {
        coordinate_range(
            &row.try_get::<String, _>("coordinate_kind").map_err(db)?,
            coordinate,
        )?
    } else {
        None
    };
    if selected.is_some_and(|range| range.end < range.start || range.end > length) {
        return Err(Error::Invalid(
            "source coordinate is outside payload".into(),
        ));
    }
    let mut provenance = vec![reference.clone()];
    if let Some(derived) = derived {
        if matches!(reference, CognitiveRef::DerivedRegion(_)) {
            provenance.push(CognitiveRef::DerivedRepresentation(
                DerivedRepresentationId(derived),
            ));
        }
        provenance.extend(
            roots
                .into_iter()
                .map(|id| CognitiveRef::SourceRegion(SourceRegionId(id))),
        );
        if text_length.is_none() {
            provenance.push(CognitiveRef::Artifact(ArtifactId(
                row.try_get("payload_artifact_id").map_err(db)?,
            )));
        }
    } else if !matches!(reference, CognitiveRef::Artifact(_)) {
        provenance.push(CognitiveRef::Artifact(ArtifactId(
            row.try_get("artifact_id").map_err(db)?,
        )));
    }
    Ok(Payload {
        text,
        hash,
        media_type,
        length,
        selected,
        provenance,
        producer: row.try_get("producer").map_err(db)?,
    })
}

fn coordinate_range(kind: &str, coordinate: &serde_json::Value) -> Result<Option<ByteRange>> {
    match kind {
        "whole_artifact" => Ok(None),
        "byte_range" | "text_span" | "description_segment" => {
            let (start_key, end_key) = if kind == "description_segment" {
                ("utf8_start", "utf8_end")
            } else {
                ("start", "end")
            };
            let start = coordinate
                .get(start_key)
                .and_then(|v| v.as_u64())
                .ok_or_else(|| Error::Invalid("coordinate start is required".into()))?;
            let end = coordinate
                .get(end_key)
                .and_then(|v| v.as_u64())
                .ok_or_else(|| Error::Invalid("coordinate end is required".into()))?;
            Ok(Some(ByteRange { start, end }))
        }
        _ => Err(Error::Unavailable(format!(
            "coordinate materializer for '{kind}' is unavailable"
        ))),
    }
}
