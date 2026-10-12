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

/// Request-scoped resolved metadata. Payload bytes are read only when consumed.
pub struct MaterialReadInput {
    reference: CognitiveRef,
    payload: Payload,
    view_digest: Option<String>,
}

impl MaterialService {
    /// Material owns its evidence and interpretation meaning in working context.
    pub async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: usize,
    ) -> Result<nous_runtime::ContextSource> {
        let material = self
            .materialize(
                subject,
                MaterializeRequest {
                    reference: reference.clone(),
                    byte_range: None,
                    max_bytes: (max_bytes.max(1) as u64).min(self.max_upload_bytes),
                    resource_handle: None,
                    resource: None,
                },
            )
            .await?;
        material_context(material, max_bytes)
    }

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
        if request.max_bytes == 0 || request.max_bytes > self.max_upload_bytes {
            return Err(Error::Invalid(
                "materialization byte bound is invalid".into(),
            ));
        }
        let input = self
            .read_inputs(subject, std::slice::from_ref(&request.reference), view)
            .await?
            .remove(&request.reference)
            .ok_or_else(|| {
                Error::Unavailable("material has no locally available payload".into())
            })?;
        let payload = input.payload;
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
        Ok(self
            .text_excerpts(subject, std::slice::from_ref(reference), max_bytes, view)
            .await?
            .remove(reference))
    }

    /// Exact selected text, with one metadata read per bounded batch. Object reads remain bounded.
    pub async fn text_excerpts(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        max_bytes: u64,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<std::collections::HashMap<CognitiveRef, MaterialTextView>> {
        if max_bytes == 0 || max_bytes > self.max_upload_bytes {
            return Err(Error::Invalid("text projection bound is invalid".into()));
        }
        let inputs = self.read_inputs(subject, references, view).await?;
        let mut result = std::collections::HashMap::new();
        for (reference, input) in inputs {
            if let Some(text) = self.read_text_input(input, max_bytes).await? {
                result.insert(reference, text);
            }
        }
        Ok(result)
    }

    /// Resolve exact Material payload, producer and provenance metadata without reading object bytes.
    #[expect(
        clippy::too_many_lines,
        reason = "one bounded metadata query resolves the five Material reference shapes"
    )]
    pub async fn read_inputs(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<std::collections::HashMap<CognitiveRef, MaterialReadInput>> {
        if references.len() > 256 {
            return Err(Error::Invalid(
                "Material text lookup supports at most 256 references".into(),
            ));
        }
        if references.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        self.store.require_subject(subject).await?;
        let mut resolved = Vec::new();
        let mut kinds = Vec::new();
        let mut ids = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for reference in references {
            if !seen.insert(reference) {
                continue;
            }
            self.require_lineage_view(subject, reference, view)?;
            let (kind, id) = match reference {
                CognitiveRef::Occurrence(id) => ("occurrence", id.0),
                CognitiveRef::Artifact(id) => ("artifact", id.0),
                CognitiveRef::SourceRegion(id) => ("source_region", id.0),
                CognitiveRef::DerivedRepresentation(id) => ("derived_representation", id.0),
                CognitiveRef::DerivedRegion(id) => ("derived_region", id.0),
                _ => {
                    self.store.validate_reference(subject, reference).await?;
                    continue;
                }
            };
            resolved.push(reference);
            kinds.push(kind);
            ids.push(id);
        }
        if resolved.is_empty() {
            return Ok(std::collections::HashMap::new());
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
SELECT r.ordinality,r.present,r.artifact_id,r.derived_id,r.coordinate_kind,r.coordinate,d.payload_text,
 a.artifact_id AS payload_artifact_id,to_jsonb(p) AS producer,
 a.content_hash,a.media_type,a.byte_length,
 ARRAY(SELECT source_region_id FROM representation_source_regions($1,r.derived_id)) AS source_roots
FROM resolved r
LEFT JOIN derived_representations d ON d.subject_id=$1 AND d.derived_representation_id=r.derived_id
LEFT JOIN artifacts a ON a.subject_id=$1 AND a.artifact_id=coalesce(r.artifact_id,d.payload_artifact_id)
LEFT JOIN producer_signatures p ON p.producer_signature_id=d.producer_signature_id
"#).bind(subject.0).bind(kinds).bind(ids).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut result = std::collections::HashMap::new();
        for row in rows {
            let index: i64 = row.try_get("ordinality").map_err(db)?;
            let reference = resolved[index as usize - 1];
            if !row.try_get::<bool, _>("present").map_err(db)? {
                return Err(Error::NotFound("cognitive reference not found".into()));
            }
            let derived: Option<Uuid> = row.try_get("derived_id").map_err(db)?;
            let roots: Vec<Uuid> = row.try_get("source_roots").map_err(db)?;
            if derived.is_some() && roots.is_empty() {
                return Err(Error::Invalid(
                    "derived representation has no source roots".into(),
                ));
            }
            let text: Option<String> = row.try_get("payload_text").map_err(db)?;
            let hash: Option<String> = row.try_get("content_hash").map_err(db)?;
            if text.is_none() && hash.is_none() {
                continue;
            }
            let (media_type, length) = if let Some(text) = &text {
                ("text/plain; charset=utf-8".to_owned(), text.len() as u64)
            } else {
                (
                    row.try_get::<String, _>("media_type").map_err(db)?,
                    row.try_get::<i64, _>("byte_length").map_err(db)? as u64,
                )
            };
            let coordinate: Option<serde_json::Value> = row.try_get("coordinate").map_err(db)?;
            let selected = if let Some(coordinate) = &coordinate {
                match coordinate_range(
                    &row.try_get::<String, _>("coordinate_kind").map_err(db)?,
                    coordinate,
                ) {
                    Ok(range) => range,
                    Err(Error::Unavailable(_)) => continue,
                    Err(error) => return Err(error),
                }
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
                if text.is_none() {
                    provenance.push(CognitiveRef::Artifact(ArtifactId(
                        row.try_get("payload_artifact_id").map_err(db)?,
                    )));
                }
            } else if !matches!(reference, CognitiveRef::Artifact(_)) {
                provenance.push(CognitiveRef::Artifact(ArtifactId(
                    row.try_get("artifact_id").map_err(db)?,
                )));
            }
            let payload = Payload {
                text,
                hash,
                media_type,
                length,
                selected,
                provenance,
                producer: row.try_get("producer").map_err(db)?,
                selection: coordinate,
            };
            result.insert(
                reference.clone(),
                MaterialReadInput {
                    reference: reference.clone(),
                    payload,
                    view_digest: view.map(|view| view.snapshot_digest.clone()),
                },
            );
        }
        Ok(result)
    }

    pub async fn read_text_input(
        &self,
        input: MaterialReadInput,
        max_bytes: u64,
    ) -> Result<Option<MaterialTextView>> {
        if max_bytes == 0 || max_bytes > self.max_upload_bytes {
            return Err(Error::Invalid("text projection bound is invalid".into()));
        }
        let reference = &input.reference;
        let payload = input.payload;
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
            view_digest: input.view_digest.clone(),
        }))
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

fn material_context(
    material: MaterializedEvidence,
    max_bytes: usize,
) -> Result<nous_runtime::ContextSource> {
    let text = if max_bytes > 0
        && matches!(
            nous_runtime::modality(&material.media_type),
            Modality::Text | Modality::Structured
        ) {
        let end = match std::str::from_utf8(&material.bytes) {
            Ok(_) => material.bytes.len(),
            Err(error) => error.valid_up_to(),
        };
        Some(
            String::from_utf8(material.bytes[..end].to_vec())
                .map_err(|e| Error::Infrastructure(e.to_string()))?,
        )
    } else {
        None
    };
    let evidence = material
        .provenance
        .into_iter()
        .map(|reference| EvidenceHandle {
            epistemic_relation: None,
            reference,
            basis_role: "source".into(),
        })
        .collect::<Vec<_>>();
    Ok(nous_runtime::ContextSource {
        source_revision: None,
        media_type: material.media_type,
        text,
        authority: if material.producer.is_some() {
            AuthorityClass::Interpretation
        } else {
            AuthorityClass::Evidence
        },
        provenance: ProvenanceSummary {
            source_count: evidence.len(),
            producer_signatures: material
                .producer
                .and_then(|p| {
                    p.get("signature_hash")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                })
                .into_iter()
                .collect(),
            note: None,
        },
        evidence,
    })
}
