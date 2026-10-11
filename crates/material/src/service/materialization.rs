// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;

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
            return Err(Error::Invalid(
                "materialization byte bound is invalid".into(),
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
            return Err(Error::Invalid(
                "requested range is outside selected source".into(),
            ));
        }
        let start = selected.start + requested.start;
        let requested_end = selected.start + requested.end;
        let mut end = requested_end.min(start.saturating_add(request.max_bytes));
        let text_media = is_text_media(&payload.media_type);
        if text_media {
            for boundary in [selected.start, selected.end, start, requested_end] {
                if !self.utf8_boundary(&payload, boundary).await? {
                    return Err(Error::Invalid(
                        "text selection must use UTF-8 byte boundaries".into(),
                    ));
                }
            }
        }
        let mut bytes = if let Some(text) = payload.text.take() {
            text.as_bytes()[start as usize..end as usize].to_vec()
        } else if let Some(hash) = payload.hash {
            self.objects.read_range(&hash, start, end).await?
        } else {
            return Err(Error::Unavailable(
                "material has no locally available payload".into(),
            ));
        };
        if text_media && let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_some() || end == requested_end {
                return Err(Error::Invalid("selected text is not valid UTF-8".into()));
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
        let hash = payload
            .hash
            .as_ref()
            .ok_or_else(|| Error::Unavailable("text payload is unavailable".into()))?;
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
            return Err(Error::Invalid("text projection bound is invalid".into()));
        }
        self.require_lineage_view(subject, reference, view)?;
        self.store.validate_reference(subject, reference).await?;
        let payload = match self.payload(subject, reference).await {
            Ok(payload) => payload,
            Err(Error::Unavailable(_)) => return Ok(None),
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
        let text = String::from_utf8(selected.bytes)
            .map_err(|_| Error::Invalid("selected text is not UTF-8".into()))?;
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

    async fn payload(&self, subject: SubjectId, reference: &CognitiveRef) -> Result<Payload> {
        let mut provenance = vec![reference.clone()];
        let mut selection = None;
        let mut selected = None;
        let mut reference = reference.clone();
        if let CognitiveRef::Occurrence(id) = reference {
            let artifact: Option<Uuid> = sqlx::query_scalar("SELECT artifact_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$2").bind(subject.0).bind(id.0).fetch_one(self.store.pool()).await.map_err(db)?;
            reference = CognitiveRef::Artifact(ArtifactId(artifact.ok_or_else(|| {
                Error::Unavailable("external occurrence requires its Host resolver".into())
            })?));
            provenance.push(reference.clone());
        }
        if let CognitiveRef::SourceRegion(id) = reference {
            let row = sqlx::query("SELECT artifact_id,coordinate_kind,coordinate FROM source_regions WHERE subject_id=$1 AND source_region_id=$2").bind(subject.0).bind(id.0).fetch_one(self.store.pool()).await.map_err(db)?;
            let coordinate: serde_json::Value = row.try_get("coordinate").map_err(db)?;
            selected = coordinate_range(
                &row.try_get::<String, _>("coordinate_kind").map_err(db)?,
                &coordinate,
            )?;
            selection = Some(coordinate);
            reference = CognitiveRef::Artifact(ArtifactId(row.try_get("artifact_id").map_err(db)?));
            provenance.push(reference.clone());
        }
        if let CognitiveRef::DerivedRegion(id) = reference {
            let row = sqlx::query("SELECT derived_representation_id,coordinate_kind,coordinate FROM derived_regions WHERE subject_id=$1 AND derived_region_id=$2").bind(subject.0).bind(id.0).fetch_one(self.store.pool()).await.map_err(db)?;
            let coordinate: serde_json::Value = row.try_get("coordinate").map_err(db)?;
            selected = coordinate_range(
                &row.try_get::<String, _>("coordinate_kind").map_err(db)?,
                &coordinate,
            )?;
            selection = Some(coordinate);
            reference = CognitiveRef::DerivedRepresentation(DerivedRepresentationId(
                row.try_get("derived_representation_id").map_err(db)?,
            ));
            provenance.push(reference.clone());
        }
        let mut payload = match reference {
            CognitiveRef::Artifact(id) => {
                let artifact = self.artifact(subject, id).await?;
                Payload {
                    text: None,
                    hash: Some(artifact.content_hash),
                    media_type: artifact.media_type,
                    length: artifact.byte_length,
                    selected: None,
                    provenance,
                    producer: None,
                    selection,
                }
            }
            CognitiveRef::DerivedRepresentation(id) => {
                self.derived_payload(subject, id, provenance, selection)
                    .await?
            }
            _ => {
                return Err(Error::Unavailable(
                    "reference needs its semantic owner or Host materializer".into(),
                ));
            }
        };
        if let Some(range) = selected {
            if range.end < range.start || range.end > payload.length {
                return Err(Error::Invalid(
                    "source coordinate is outside payload".into(),
                ));
            }
            payload.selected = Some(range);
        }
        Ok(payload)
    }

    async fn derived_payload(
        &self,
        subject: SubjectId,
        id: DerivedRepresentationId,
        mut provenance: Vec<CognitiveRef>,
        selection: Option<serde_json::Value>,
    ) -> Result<Payload> {
        let row=sqlx::query("SELECT d.payload_text,d.payload_artifact_id,to_jsonb(p) AS producer FROM derived_representations d JOIN producer_signatures p USING(producer_signature_id) WHERE d.subject_id=$1 AND d.derived_representation_id=$2")
            .bind(subject.0).bind(id.0).fetch_one(self.store.pool()).await.map_err(db)?;
        let roots: Vec<Uuid> =
            sqlx::query_scalar("SELECT source_region_id FROM representation_source_regions($1,$2)")
                .bind(subject.0)
                .bind(id.0)
                .fetch_all(self.store.pool())
                .await
                .map_err(db)?;
        if roots.is_empty() {
            return Err(Error::Invalid(
                "derived representation has no source roots".into(),
            ));
        }
        provenance.extend(
            roots
                .into_iter()
                .map(|id| CognitiveRef::SourceRegion(SourceRegionId(id))),
        );
        let text: Option<String> = row.try_get("payload_text").map_err(db)?;
        let (hash, media_type, length) = if let Some(text) = &text {
            (None, "text/plain; charset=utf-8".into(), text.len() as u64)
        } else {
            let artifact = self
                .artifact(
                    subject,
                    ArtifactId(row.try_get("payload_artifact_id").map_err(db)?),
                )
                .await?;
            provenance.push(CognitiveRef::Artifact(artifact.artifact_id));
            (
                Some(artifact.content_hash),
                artifact.media_type,
                artifact.byte_length,
            )
        };
        Ok(Payload {
            text,
            hash,
            media_type,
            length,
            selected: None,
            provenance,
            producer: Some(row.try_get("producer").map_err(db)?),
            selection,
        })
    }
}

fn is_text_media(media: &str) -> bool {
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
