// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializeRequest {
    pub reference: CognitiveRef,
    pub byte_range: Option<ByteRange>,
    #[serde(default = "default_read_bound")]
    pub max_bytes: u64,
    pub resource_handle: Option<String>,
    pub resource: Option<ResourceRef>,
}

fn default_read_bound() -> u64 {
    1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializedEvidence {
    pub reference: CognitiveRef,
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub byte_range: ByteRange,
    pub total_bytes: u64,
    pub partial: bool,
    pub provenance: Vec<CognitiveRef>,
    pub producer: Option<serde_json::Value>,
    pub selection: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialTextView {
    pub reference: CognitiveRef,
    pub text: String,
    pub content_identity: String,
    pub selection: ByteRange,
    pub total_bytes: u64,
    pub view_digest: Option<String>,
}

struct Payload {
    text: Option<String>,
    hash: Option<String>,
    media_type: String,
    length: u64,
    selected: Option<ByteRange>,
    provenance: Vec<CognitiveRef>,
    producer: Option<serde_json::Value>,
    selection: Option<serde_json::Value>,
}

impl MaterialService {
    pub async fn materialize(
        &self,
        subject: SubjectId,
        request: MaterializeRequest,
    ) -> Result<MaterializedEvidence> {
        self.materialize_in_view(subject, request, None).await
    }

    pub async fn materialize_in_view(
        &self,
        subject: SubjectId,
        request: MaterializeRequest,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<MaterializedEvidence> {
        self.require_lineage_view(subject, &request.reference, view)?;
        if request.max_bytes == 0 || request.max_bytes > self.max_upload_bytes {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "materialization byte bound is invalid",
            ));
        }
        self.store
            .validate_reference(subject, &request.reference)
            .await?;
        let payload = self.payload(subject, &request.reference).await?;
        self.materialize_payload(request, payload).await
    }

    async fn materialize_payload(
        &self,
        request: MaterializeRequest,
        mut payload: Payload,
    ) -> Result<MaterializedEvidence> {
        let selected = payload.selected.unwrap_or(ByteRange {
            start: 0,
            end: payload.length,
        });
        let requested = request.byte_range.unwrap_or(ByteRange {
            start: 0,
            end: selected.end - selected.start,
        });
        if requested.end < requested.start || requested.end > selected.end - selected.start {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "requested range is outside selected source",
            ));
        }
        let start = selected.start + requested.start;
        let requested_end = selected.start + requested.end;
        let mut end = requested_end.min(start.saturating_add(request.max_bytes));
        let text_media = is_text_media(&payload.media_type);
        if text_media {
            for boundary in [selected.start, selected.end, start, requested_end] {
                if !self.utf8_boundary(&payload, boundary).await? {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Material,
                        nous_core::ErrorCode::InvalidRequest,
                        "text selection must use UTF-8 byte boundaries",
                    ));
                }
            }
        }
        let mut bytes = if let Some(text) = payload.text.take() {
            text.as_bytes()[start as usize..end as usize].to_vec()
        } else if let Some(hash) = payload.hash {
            self.objects.read_range(&hash, start, end).await?
        } else {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::Unavailable,
                "material has no locally available payload",
            ));
        };
        if text_media && let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_some() || end == requested_end {
                return Err(Error::new(
                    nous_core::ErrorOwner::Material,
                    nous_core::ErrorCode::InvalidRequest,
                    "selected text is not valid UTF-8",
                ));
            }
            bytes.truncate(error.valid_up_to());
            end = start + bytes.len() as u64;
        }
        Ok(MaterializedEvidence {
            reference: request.reference,
            media_type: payload.media_type,
            bytes,
            byte_range: ByteRange { start, end },
            total_bytes: payload.length,
            partial: start > 0 || end < payload.length,
            provenance: payload.provenance,
            producer: payload.producer,
            selection: payload.selection,
        })
    }

    async fn utf8_boundary(&self, payload: &Payload, offset: u64) -> Result<bool> {
        if offset == 0 || offset == payload.length {
            return Ok(true);
        }
        if let Some(text) = &payload.text {
            return Ok(text.is_char_boundary(offset as usize));
        }
        let hash = payload.hash.as_ref().ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::Unavailable,
                "text payload is unavailable",
            )
        })?;
        let bytes = self.objects.read_range(hash, offset, offset + 1).await?;
        Ok(bytes.first().is_some_and(|byte| byte & 0xc0 != 0x80))
    }

    pub async fn text_view(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Option<MaterialTextView>> {
        self.text_excerpt(subject, reference, self.max_upload_bytes, view)
            .await
    }

    pub async fn text_excerpt(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: u64,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Option<MaterialTextView>> {
        if max_bytes == 0 || max_bytes > self.max_upload_bytes {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "text projection bound is invalid",
            ));
        }
        self.require_lineage_view(subject, reference, view)?;
        self.store.validate_reference(subject, reference).await?;
        let payload = match self.payload(subject, reference).await {
            Ok(payload) => payload,
            Err(Error {
                code: nous_core::ErrorCode::Unavailable,
                ..
            }) => return Ok(None),
            Err(error) => return Err(error),
        };
        if !is_text_media(&payload.media_type) {
            return Ok(None);
        }
        let selected = self
            .materialize_payload(
                MaterializeRequest {
                    reference: reference.clone(),
                    byte_range: None,
                    max_bytes,
                    resource_handle: None,
                    resource: None,
                },
                payload,
            )
            .await?;
        let text = String::from_utf8(selected.bytes).map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "selected text is not UTF-8",
            )
            .with_source(error)
        })?;
        let content_identity = text_content_identity(&text);
        Ok(Some(MaterialTextView {
            reference: reference.clone(),
            text,
            content_identity,
            selection: selected.byte_range,
            total_bytes: selected.total_bytes,
            view_digest: view.map(|view| view.snapshot_digest.clone()),
        }))
    }

    /// Materialize a bounded set of owner-visible references after resolving
    /// their payload lineage in one grouped Authority read. Object-store range
    /// reads remain per payload, but semantic rows are not fetched per ref.
    pub async fn materialize_batch(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        max_bytes: u64,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, MaterializedEvidence>> {
        if references.len() > 256 || max_bytes == 0 || max_bytes > self.max_upload_bytes {
            return Err(Error::new(
                ErrorOwner::Material,
                ErrorCode::InvalidRequest,
                "Material context batch exceeds bounds",
            ));
        }
        let mut payloads = self.payloads(subject, references, view).await?;
        let mut remaining = max_bytes;
        let mut output = HashMap::new();
        for reference in references {
            if remaining == 0 {
                break;
            }
            let Some(payload) = payloads.remove(reference) else {
                continue;
            };
            let material = self
                .materialize_payload(
                    MaterializeRequest {
                        reference: reference.clone(),
                        byte_range: None,
                        max_bytes: remaining,
                        resource_handle: None,
                        resource: None,
                    },
                    payload,
                )
                .await?;
            remaining = remaining.saturating_sub(material.bytes.len() as u64);
            output.insert(reference.clone(), material);
        }
        Ok(output)
    }

    async fn payload(&self, subject: SubjectId, reference: &CognitiveRef) -> Result<Payload> {
        self.payloads(subject, std::slice::from_ref(reference), None)
            .await?
            .remove(reference)
            .ok_or_else(|| {
                Error::new(
                    ErrorOwner::Material,
                    ErrorCode::Unavailable,
                    "reference requires its semantic owner or Host materializer",
                )
            })
    }

    async fn payloads(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, Payload>> {
        let mut kinds = Vec::with_capacity(references.len());
        let mut ids = Vec::with_capacity(references.len());
        let mut supported_references = Vec::with_capacity(references.len());
        for reference in references {
            if let Some(view) = view {
                if view.subject != subject {
                    return Err(Error::new(
                        ErrorOwner::Material,
                        ErrorCode::InvalidRequest,
                        "Material context view belongs to another subject",
                    ));
                }
                if !view.material_contains(reference) {
                    continue;
                }
            }
            let (kind, id) = match reference {
                CognitiveRef::Artifact(id) => (0, id.0),
                CognitiveRef::Occurrence(id) => (1, id.0),
                CognitiveRef::SourceRegion(id) => (2, id.0),
                CognitiveRef::DerivedRepresentation(id) => (3, id.0),
                CognitiveRef::DerivedRegion(id) => (4, id.0),
                _ => continue,
            };
            kinds.push(kind);
            ids.push(id);
            supported_references.push(reference.clone());
        }
        if kinds.is_empty() {
            return Ok(HashMap::new());
        }
        let mut tx = self.store.pool().begin().await.map_err(db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let rows = sqlx::query("WITH requested AS (SELECT ord,kind,reference_id FROM unnest($1::int[],$2::uuid[]) WITH ORDINALITY AS q(kind,reference_id,ord)) SELECT requested.ord,requested.kind,requested.reference_id,occurrence.artifact_id AS occurrence_artifact_id,source_region.artifact_id AS source_artifact_id,source_region.coordinate_kind AS source_coordinate_kind,source_region.coordinate AS source_coordinate,derived_region.derived_representation_id AS region_representation_id,derived_region.coordinate_kind AS derived_coordinate_kind,derived_region.coordinate AS derived_coordinate,derived.derived_representation_id AS representation_id,derived.payload_text,derived.payload_artifact_id,to_jsonb(producer) AS producer,roots.source_region_ids,artifact.artifact_id,artifact.content_hash,artifact.byte_length,artifact.media_type FROM requested LEFT JOIN observation_occurrences occurrence ON requested.kind=1 AND occurrence.subject_id=$3 AND occurrence.occurrence_id=requested.reference_id LEFT JOIN source_regions source_region ON requested.kind=2 AND source_region.subject_id=$3 AND source_region.source_region_id=requested.reference_id LEFT JOIN derived_regions derived_region ON requested.kind=4 AND derived_region.subject_id=$3 AND derived_region.derived_region_id=requested.reference_id LEFT JOIN derived_representations derived ON derived.subject_id=$3 AND derived.derived_representation_id=CASE WHEN requested.kind=3 THEN requested.reference_id WHEN requested.kind=4 THEN derived_region.derived_representation_id ELSE NULL END LEFT JOIN producer_signatures producer ON producer.producer_signature_id=derived.producer_signature_id LEFT JOIN LATERAL (SELECT array_agg(source_region_id ORDER BY source_region_id) AS source_region_ids FROM representation_source_regions($3,derived.derived_representation_id)) roots ON derived.derived_representation_id IS NOT NULL LEFT JOIN artifacts artifact ON artifact.subject_id=$3 AND artifact.artifact_id=CASE WHEN requested.kind=0 THEN requested.reference_id WHEN requested.kind=1 THEN occurrence.artifact_id WHEN requested.kind=2 THEN source_region.artifact_id WHEN requested.kind IN (3,4) THEN derived.payload_artifact_id ELSE NULL END ORDER BY requested.ord")
            .bind(&kinds)
            .bind(&ids)
            .bind(subject.0)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;

        let mut payloads = HashMap::new();
        for (reference, row) in supported_references.into_iter().zip(rows) {
            let kind: i32 = row.try_get("kind").map_err(db)?;
            let mut provenance = vec![reference.clone()];
            let mut selection = None;
            let mut selected = None;
            if kind == 2 {
                let Some(artifact_id) = row
                    .try_get::<Option<Uuid>, _>("source_artifact_id")
                    .map_err(db)?
                else {
                    continue;
                };
                let coordinate: serde_json::Value = row.try_get("source_coordinate").map_err(db)?;
                selected = coordinate_range(
                    &row.try_get::<String, _>("source_coordinate_kind")
                        .map_err(db)?,
                    &coordinate,
                )?;
                selection = Some(coordinate);
                provenance.push(CognitiveRef::Artifact(ArtifactId(artifact_id)));
            }
            if kind == 4 {
                let Some(representation_id) = row
                    .try_get::<Option<Uuid>, _>("region_representation_id")
                    .map_err(db)?
                else {
                    continue;
                };
                let coordinate: serde_json::Value =
                    row.try_get("derived_coordinate").map_err(db)?;
                selected = coordinate_range(
                    &row.try_get::<String, _>("derived_coordinate_kind")
                        .map_err(db)?,
                    &coordinate,
                )?;
                selection = Some(coordinate);
                provenance.push(CognitiveRef::DerivedRepresentation(
                    DerivedRepresentationId(representation_id),
                ));
            }
            if kind == 1 {
                let artifact_id: Option<Uuid> =
                    { row.try_get("occurrence_artifact_id").map_err(db)? };
                if let Some(id) = artifact_id {
                    provenance.push(CognitiveRef::Artifact(ArtifactId(id)));
                } else {
                    continue;
                }
            }
            if matches!(kind, 3 | 4) {
                let Some(_representation_id) = row
                    .try_get::<Option<Uuid>, _>("representation_id")
                    .map_err(db)?
                else {
                    continue;
                };
                let roots = row
                    .try_get::<Option<Vec<Uuid>>, _>("source_region_ids")
                    .map_err(db)?
                    .unwrap_or_default();
                if roots.is_empty() {
                    return Err(Error::new(
                        ErrorOwner::Material,
                        ErrorCode::InvalidRequest,
                        "derived representation has no source roots",
                    ));
                }
                provenance.extend(
                    roots
                        .into_iter()
                        .map(|id| CognitiveRef::SourceRegion(SourceRegionId(id))),
                );
                if let Some(id) = row.try_get::<Option<Uuid>, _>("artifact_id").map_err(db)? {
                    provenance.push(CognitiveRef::Artifact(ArtifactId(id)));
                }
            }
            let text: Option<String> = row.try_get("payload_text").map_err(db)?;
            let artifact_id: Option<Uuid> = row.try_get("artifact_id").map_err(db)?;
            let (hash, media_type, length) = if let Some(text) = &text {
                (
                    None,
                    "text/plain; charset=utf-8".to_owned(),
                    text.len() as u64,
                )
            } else if artifact_id.is_some() {
                let hash: String = row.try_get("content_hash").map_err(db)?;
                let bytes: i64 = row.try_get("byte_length").map_err(db)?;
                let media: String = row.try_get("media_type").map_err(db)?;
                (Some(hash), media, bytes as u64)
            } else {
                continue;
            };
            if let Some(range) = selected
                && (range.end < range.start || range.end > length)
            {
                return Err(Error::new(
                    ErrorOwner::Material,
                    ErrorCode::InvalidRequest,
                    "Material source coordinate is outside payload",
                ));
            }
            let payload = Payload {
                text,
                hash,
                media_type,
                length,
                selected,
                provenance,
                producer: row.try_get("producer").map_err(db)?,
                selection,
            };
            payloads.insert(reference, payload);
        }

        Ok(payloads)
    }
}

pub(super) fn is_text_media(media: &str) -> bool {
    media.starts_with("text/") || media.contains("json") || media.contains("xml")
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
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Material,
                        nous_core::ErrorCode::InvalidRequest,
                        "coordinate start is required",
                    )
                })?;
            let end = coordinate
                .get(end_key)
                .and_then(|v| v.as_u64())
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Material,
                        nous_core::ErrorCode::InvalidRequest,
                        "coordinate end is required",
                    )
                })?;
            Ok(Some(ByteRange { start, end }))
        }
        _ => Err(Error::new(
            nous_core::ErrorOwner::Material,
            nous_core::ErrorCode::Unavailable,
            "Coordinate materializer is unavailable",
        )),
    }
}
