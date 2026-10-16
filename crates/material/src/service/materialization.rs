// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::lineage::evidence_reference;
use super::read::{MaterialReadInput, Payload};
use super::*;
use futures::{StreamExt, stream::BoxStream};
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

impl MaterialService {
    /// Material owns its evidence and interpretation meaning in working context.
    pub async fn context_sources(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        max_bytes: usize,
        source_read: Option<&SourceReadView>,
    ) -> Result<Vec<Result<nous_runtime::ContextSource>>> {
        self.context_sources_with_paths(subject, references, max_bytes, source_read, None)
            .await
    }

    /// Current actual Occurrence/locator paths are selections, never additional grants.
    pub async fn current_material_sources(
        &self,
        subject: SubjectId,
        paths: &[EvidenceRef],
        max_bytes: usize,
        source_read: &SourceReadView,
    ) -> Result<Vec<Result<nous_runtime::ContextSource>>> {
        if paths.len() > 128 || source_read.subject != subject {
            return Err(Error::Invalid(
                "current material source envelope is invalid".into(),
            ));
        }
        let references = paths
            .iter()
            .map(EvidenceRef::cognitive_ref)
            .collect::<Vec<_>>();
        self.context_sources_with_paths(
            subject,
            &references,
            max_bytes,
            Some(source_read),
            Some(paths),
        )
        .await
    }

    async fn context_sources_with_paths(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        max_bytes: usize,
        source_read: Option<&SourceReadView>,
        paths: Option<&[EvidenceRef]>,
    ) -> Result<Vec<Result<nous_runtime::ContextSource>>> {
        self.store.require_subject(subject).await?;
        let mut tx = self.store.read_view().await?;
        let mut admitted = Vec::new();
        let mut allowed = Vec::new();
        for (index, reference) in references.iter().enumerate() {
            let permitted = if let Some(paths) = paths {
                let read = source_read.expect("paired reads require scope");
                let path = &paths[index];
                let (kind, value) = reference_parts(reference);
                AuthorityStore::source_read_allowed_in(
                    &mut tx,
                    read,
                    &CognitiveRef::Occurrence(path.occurrence_id),
                )
                .await?
                    && sqlx::query_scalar::<_, bool>(
                        "SELECT material_source_read_allowed($1,$2,$3,$4,$5,$6)",
                    )
                    .bind(subject.0)
                    .bind(kind)
                    .bind(value)
                    .bind(&read.origin_conversation_refs)
                    .bind(serde_json::json!(&read.explicit_refs))
                    .bind(path.occurrence_id.0)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db)?
            } else if let Some(read) = source_read {
                AuthorityStore::source_read_allowed_in(&mut tx, read, reference).await?
            } else {
                true
            };
            allowed.push(permitted);
            if permitted {
                admitted.push(reference.clone());
            }
        }
        let mut inputs = self
            .read_inputs_in(&mut tx, subject, &admitted, None, max_bytes > 0)
            .await?;
        let metadata = perception_metadata_in(&mut tx, subject, &admitted).await?;
        let occurrence_ids = references
            .iter()
            .enumerate()
            .filter(|(index, _)| allowed[*index])
            .filter_map(|(index, reference)| {
                paths.map(|paths| paths[index].occurrence_id).or_else(|| {
                    if let CognitiveRef::Occurrence(id) = reference {
                        Some(*id)
                    } else {
                        None
                    }
                })
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let occurrences = self
            .occurrences_in(&mut tx, subject, &occurrence_ids, None)
            .await?;
        tx.commit().await.map_err(db)?;
        let mut results = Vec::with_capacity(references.len());
        for (index, reference) in references.iter().enumerate() {
            let input = if !allowed[index] {
                None
            } else if references[index + 1..].contains(reference) {
                inputs.get(reference).cloned()
            } else {
                inputs.remove(reference)
            };
            let occurrence = paths
                .map(|paths| paths[index].occurrence_id)
                .or_else(|| {
                    if let CognitiveRef::Occurrence(id) = reference {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .and_then(|id| occurrences.get(&id));
            let temporal = if allowed[index] {
                let data = metadata.get(reference);
                let formed_at = data
                    .and_then(|data| data.get("formed_stamp"))
                    .filter(|value| !value.is_null())
                    .map(|value| serde_json::from_value(value.clone()))
                    .transpose()
                    .map_err(|e| Error::Infrastructure(e.to_string()))?;
                let recorded_at = data
                    .and_then(|data| data.get("recorded_at"))
                    .map(|value| serde_json::from_value(value.clone()))
                    .transpose()
                    .map_err(|e| Error::Infrastructure(e.to_string()))?;
                Some(FreshnessDescriptor {
                    temporal_evidence: TemporalEvidence {
                        sources: occurrence
                            .into_iter()
                            .map(|o| SourceTemporalEvidence {
                                occurrence_id: o.occurrence_id,
                                occurred: DomainTemporalExtent {
                                    time_domain_ref: o.occurred_time_domain_ref.clone(),
                                    extent: o.occurred_time.clone(),
                                },
                                observed: o.observed_stamp.clone(),
                                observed_at: o.observed_at,
                                recorded_at: o.created_at,
                            })
                            .collect(),
                    },
                    valid_time: DomainTemporalExtent::default(),
                    formed_at,
                    recorded_at,
                })
            } else {
                None
            };
            let result = self
                .context_source_from_input(
                    input,
                    reference,
                    max_bytes,
                    paths.map(|p| &p[index]),
                    metadata.get(reference),
                    temporal,
                )
                .await;
            results.push(result);
        }
        Ok(results)
    }

    async fn context_source_from_input(
        &self,
        input: Option<MaterialReadInput>,
        reference: &CognitiveRef,
        max_bytes: usize,
        path: Option<&EvidenceRef>,
        metadata: Option<&serde_json::Value>,
        temporal: Option<FreshnessDescriptor>,
    ) -> Result<nous_runtime::ContextSource> {
        let mut payload = input
            .ok_or_else(|| Error::Unavailable("material metadata is unavailable".into()))?
            .payload?;
        let media_type = payload.media_type.clone();
        let producer = payload.producer.take();
        let provenance = std::mem::take(&mut payload.provenance);
        let length = payload
            .selected
            .map_or(payload.length, |range| range.end - range.start);
        if max_bytes > 0
            && matches!(
                media_modality(&media_type),
                Modality::Text | Modality::Structured
            )
            && length > max_bytes as u64
        {
            return Err(Error::Unavailable(
                "complete Material semantic unit exceeds projection budget".into(),
            ));
        }
        let mut text = if max_bytes > 0
            && matches!(
                media_modality(&media_type),
                Modality::Text | Modality::Structured
            ) {
            let selected = self
                .materialize_payload(
                    MaterializeRequest {
                        reference: reference.clone(),
                        byte_range: None,
                        max_bytes: max_bytes as u64,
                    },
                    payload,
                )
                .await?;
            let end = std::str::from_utf8(&selected.bytes)
                .map_or_else(|error| error.valid_up_to(), str::len);
            Some(
                String::from_utf8(selected.bytes[..end].to_vec())
                    .map_err(|error| Error::Infrastructure(error.to_string()))?,
            )
        } else {
            None
        };
        let mut evidence: Vec<_> = provenance
            .into_iter()
            .map(|reference| EvidenceHandle {
                epistemic_relation: None,
                reference,
                basis_role: "source".into(),
            })
            .collect();
        if let Some(path) = path
            && let Some(body) = text.as_mut()
        {
            let quality = metadata.cloned().map(|mut value| {
                if let Some(fields) = value.as_object_mut() {
                    fields.remove("formed_stamp");
                    fields.remove("recorded_at");
                }
                value
            });
            let provenance = serde_json::json!({"actual_occurrence":path.occurrence_id.0,"material_reference":reference.to_string(),"current_access":"representation","quality":quality});
            *body = format!(
                "Current actual material perception {}\n{}",
                provenance, body
            );
            if body.len() > max_bytes {
                return Err(Error::Unavailable(
                    "complete current material perception exceeds context budget".into(),
                ));
            }
        }
        if let Some(path) = path {
            evidence.push(EvidenceHandle {
                reference: CognitiveRef::Occurrence(path.occurrence_id),
                basis_role: "actual_occurrence".into(),
                epistemic_relation: None,
            });
        }
        Ok(nous_runtime::ContextSource {
            temporal,
            usage: "task_evidence",
            required: false,
            reference: reference.clone(),
            source_revision: None,
            text,
            authority: if producer.is_some() {
                AuthorityClass::Interpretation
            } else {
                AuthorityClass::Evidence
            },
            evidence,
        })
    }

    /// Select exact formation evidence and resolved mentions in one captured read.
    pub async fn formation_input(
        &self,
        subject: SubjectId,
        occurrence: OccurrenceId,
        representation: Option<DerivedRepresentationId>,
        audio_description: bool,
        include_mentions: bool,
        max_bytes: u64,
    ) -> Result<(MaterializedEvidence, Vec<(Uuid, String, EntityRef)>)> {
        if max_bytes == 0 || max_bytes > self.max_upload_bytes {
            return Err(Error::Invalid(
                "formation source byte bound exceeded".into(),
            ));
        }
        let mut tx = self.store.read_view().await?;
        let row = sqlx::query("SELECT o.artifact_id,a.media_type FROM observation_occurrences o LEFT JOIN artifacts a USING(artifact_id) WHERE o.subject_id=$1 AND o.occurrence_id=$2")
            .bind(subject.0).bind(occurrence.0).fetch_optional(&mut *tx).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("formation Occurrence not found".into()))?;
        let artifact: Option<Uuid> = row.try_get("artifact_id").map_err(db)?;
        let representation = if representation.is_some() {
            representation
        } else {
            let media: Option<String> = row.try_get("media_type").map_err(db)?;
            let kind = match media.as_deref().map(media_modality) {
                Some(Modality::Image) => "image_description",
                Some(Modality::Audio) if audio_description => "audio_description",
                Some(Modality::Audio) => "transcript",
                Some(Modality::Video) => "scene_description",
                _ => "extracted_text",
            };
            sqlx::query_scalar::<_,Uuid>("SELECT d.derived_representation_id FROM derived_representations d WHERE d.subject_id=$1 AND d.representation_kind=$2 AND EXISTS(SELECT 1 FROM representation_source_regions($1,d.derived_representation_id) roots JOIN source_regions s USING(source_region_id) WHERE s.artifact_id=$3) AND NOT EXISTS(SELECT 1 FROM derived_representations newer WHERE newer.supersedes=d.derived_representation_id) ORDER BY d.derived_representation_id DESC LIMIT 1")
                .bind(subject.0).bind(kind).bind(artifact).fetch_optional(&mut *tx).await.map_err(db)?.map(DerivedRepresentationId)
        };
        let reference = representation.map_or(
            CognitiveRef::Occurrence(occurrence),
            CognitiveRef::DerivedRepresentation,
        );
        let input = self
            .read_inputs_in(
                &mut tx,
                subject,
                std::slice::from_ref(&reference),
                None,
                true,
            )
            .await?
            .remove(&reference)
            .ok_or_else(|| Error::Unavailable("formation source unavailable".into()))?;
        let payload = input.payload?;
        if representation.is_some()
            && artifact.is_none_or(|id| !input.source_artifacts.contains(&id))
        {
            return Err(Error::Invalid(
                "formation representation does not derive from the Occurrence's Artifact".into(),
            ));
        }
        let mentions = if include_mentions {
            self.resolved_mentions_in(&mut tx, subject, occurrence)
                .await?
        } else {
            vec![]
        };
        tx.commit().await.map_err(db)?;
        let source = self
            .materialize_payload(
                MaterializeRequest {
                    reference,
                    byte_range: None,
                    max_bytes,
                },
                payload,
            )
            .await?;
        Ok((source, mentions))
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
        if request.max_bytes == 0 {
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
        let payload = input.payload?;
        self.materialize_payload(request, payload).await
    }

    async fn materialize_payload(
        &self,
        request: MaterializeRequest,
        mut payload: Payload,
    ) -> Result<MaterializedEvidence> {
        let (range, partial) = self.selected_range(&request, &payload).await?;
        let ByteRange { start, end } = range;
        let text_media = is_text_media(&payload.media_type);
        let bytes = if let Some(text) = payload.text.take() {
            text.as_bytes()[start as usize..end as usize].to_vec()
        } else if let Some(hash) = payload.hash {
            self.objects.read_range(&hash, start, end).await?
        } else {
            return Err(Error::Unavailable(
                "material has no locally available payload".into(),
            ));
        };
        if text_media {
            std::str::from_utf8(&bytes)
                .map_err(|_| Error::Invalid("selected text is not valid UTF-8".into()))?;
        }
        Ok(MaterializedEvidence {
            reference: request.reference,
            media_type: payload.media_type,
            bytes,
            byte_range: ByteRange { start, end },
            total_bytes: payload.length,
            partial,
            provenance: payload.provenance,
            producer: payload.producer,
        })
    }

    /// Metadata and exact selected bytes share the same range/UTF-8 contract as unary reads.
    pub async fn materialize_stream(
        &self,
        subject: SubjectId,
        request: MaterializeRequest,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<(MaterializedEvidence, BoxStream<'static, Result<Vec<u8>>>)> {
        if request.max_bytes == 0 {
            return Err(Error::Invalid(
                "materialization byte bound is invalid".into(),
            ));
        }
        let mut payload = self
            .read_inputs(subject, std::slice::from_ref(&request.reference), view)
            .await?
            .remove(&request.reference)
            .ok_or_else(|| Error::Unavailable("material metadata is unavailable".into()))?
            .payload?;
        let (range, partial) = self.selected_range(&request, &payload).await?;
        let stream = if let Some(text) = payload.text.take() {
            futures::stream::unfold(
                (text, range.start as usize, range.end as usize),
                |(text, start, end)| async move {
                    if start == end {
                        return None;
                    }
                    let next = (start + 262_144).min(end);
                    Some((Ok(text.as_bytes()[start..next].to_vec()), (text, next, end)))
                },
            )
            .boxed()
        } else if let Some(hash) = payload.hash.take() {
            let selected =
                (range.start != 0 || range.end != payload.length).then_some(range.start..range.end);
            self.objects.stream(&hash, selected).await?.boxed()
        } else {
            return Err(Error::Unavailable("material payload is unavailable".into()));
        };
        let stream = if is_text_media(&payload.media_type) {
            validated_text_stream(stream)
        } else {
            stream
        };
        let header = MaterializedEvidence {
            reference: request.reference,
            media_type: payload.media_type,
            bytes: Vec::new(),
            byte_range: range,
            total_bytes: payload.length,
            partial,
            provenance: payload.provenance,
            producer: payload.producer,
        };
        Ok((header, stream))
    }

    async fn selected_range(
        &self,
        request: &MaterializeRequest,
        payload: &Payload,
    ) -> Result<(ByteRange, bool)> {
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
                if !self.utf8_boundary(payload, boundary).await? {
                    return Err(Error::Invalid(
                        "text selection must use UTF-8 byte boundaries".into(),
                    ));
                }
            }
        }
        if text_media {
            for _ in 0..3 {
                if end == start || self.utf8_boundary(payload, end).await? {
                    break;
                }
                end -= 1;
            }
            if !self.utf8_boundary(payload, end).await? {
                return Err(Error::Invalid("selected text is not valid UTF-8".into()));
            }
        }
        Ok((ByteRange { start, end }, end < requested_end))
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

    pub async fn text_excerpt(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: u64,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Option<MaterialTextView>> {
        if max_bytes == 0 {
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
        if max_bytes == 0 {
            return Err(Error::Invalid("text projection bound is invalid".into()));
        }
        self.read_text_inputs(
            self.read_inputs(subject, references, view).await?,
            max_bytes,
        )
        .await
    }

    pub(super) async fn read_text_inputs(
        &self,
        inputs: std::collections::HashMap<CognitiveRef, MaterialReadInput>,
        max_bytes: u64,
    ) -> Result<std::collections::HashMap<CognitiveRef, MaterialTextView>> {
        let mut result = std::collections::HashMap::new();
        for (reference, input) in inputs {
            if let Some(text) = self.read_text_input(input, max_bytes).await? {
                result.insert(reference, text);
            }
        }
        Ok(result)
    }

    /// Complete source text for exact evidence locators; metadata and payloads are reused within the batch.
    pub async fn evidence_text_views(
        &self,
        subject: SubjectId,
        evidence: &[EvidenceRef],
        max_bytes: u64,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<(ObservationOccurrence, Option<MaterialTextView>)>> {
        if evidence.len() > 256 {
            return Err(Error::Invalid(
                "evidence batch exceeds 256 references".into(),
            ));
        }
        let mut tx = self.store.read_view().await?;
        let occurrences = self
            .occurrences_in(
                &mut tx,
                subject,
                &evidence
                    .iter()
                    .map(|evidence| evidence.occurrence_id)
                    .collect::<Vec<_>>(),
                view,
            )
            .await?;
        let references: Vec<_> = evidence.iter().map(evidence_reference).collect();
        let mut inputs = self
            .read_inputs_in(&mut tx, subject, &references, view, true)
            .await?;
        tx.commit().await.map_err(db)?;
        let mut text = std::collections::HashMap::new();
        for (evidence, reference) in evidence.iter().zip(&references) {
            let occurrence = &occurrences[&evidence.occurrence_id];
            if let Some(Err(error)) = inputs.get(reference).map(|input| &input.payload)
                && !matches!(error, Error::Unavailable(_))
            {
                return Err(error.clone());
            }
            if !matches!(evidence.locator, EvidenceLocator::WholeOccurrence)
                && !inputs.get(reference).is_some_and(|input| {
                    occurrence
                        .artifact_id
                        .is_some_and(|artifact| input.source_artifacts.contains(&artifact.0))
                })
            {
                return Err(Error::Invalid(
                    "evidence locator source does not match occurrence".into(),
                ));
            }
        }
        for (reference, input) in inputs.drain() {
            text.insert(reference, self.read_text_input(input, max_bytes).await?);
        }
        Ok(evidence
            .iter()
            .zip(references)
            .map(|(evidence, reference)| {
                (
                    occurrences[&evidence.occurrence_id].clone(),
                    text.get(&reference).cloned().flatten(),
                )
            })
            .collect())
    }

    pub async fn read_text_input(
        &self,
        input: MaterialReadInput,
        max_bytes: u64,
    ) -> Result<Option<MaterialTextView>> {
        if max_bytes == 0 {
            return Err(Error::Invalid("text projection bound is invalid".into()));
        }
        let reference = &input.reference;
        let payload = match input.payload {
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

fn validated_text_stream(
    stream: BoxStream<'static, Result<Vec<u8>>>,
) -> BoxStream<'static, Result<Vec<u8>>> {
    futures::stream::try_unfold(
        (stream, Vec::new()),
        |(mut stream, mut pending)| async move {
            loop {
                let Some(chunk) = stream.next().await else {
                    if !pending.is_empty() {
                        return Err(Error::Invalid("selected text is not valid UTF-8".into()));
                    }
                    return Ok(None);
                };
                pending.extend(chunk?);
                let end = match std::str::from_utf8(&pending) {
                    Ok(_) => pending.len(),
                    Err(error) if error.error_len().is_none() => error.valid_up_to(),
                    Err(_) => {
                        return Err(Error::Invalid("selected text is not valid UTF-8".into()));
                    }
                };
                if end == 0 {
                    continue;
                }
                let rest = pending.split_off(end);
                return Ok(Some((pending, (stream, rest))));
            }
        },
    )
    .boxed()
}

fn is_text_media(media: &str) -> bool {
    media.starts_with("text/") || media.contains("json") || media.contains("xml")
}

async fn perception_metadata_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    references: &[CognitiveRef],
) -> Result<std::collections::HashMap<CognitiveRef, serde_json::Value>> {
    let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
    let rows=sqlx::query(r#"WITH refs AS (SELECT * FROM unnest($2::text[],$3::text[]) q(kind,value))
SELECT q.kind,q.value,d.quality,d.formed_stamp,COALESCE(g.created_at,d.created_at) created_at FROM refs q LEFT JOIN derived_regions g ON q.kind='derived_region' AND g.subject_id=$1 AND g.derived_region_id::text=q.value JOIN derived_representations d ON d.subject_id=$1 AND ((q.kind='derived_representation' AND d.derived_representation_id::text=q.value) OR d.derived_representation_id=g.derived_representation_id)
UNION ALL SELECT q.kind,q.value,'{}'::jsonb,NULL::jsonb,o.created_at FROM refs q JOIN observation_occurrences o ON q.kind='occurrence' AND o.subject_id=$1 AND o.occurrence_id::text=q.value
UNION ALL SELECT q.kind,q.value,'{}'::jsonb,NULL::jsonb,a.created_at FROM refs q JOIN artifacts a ON q.kind='artifact' AND a.subject_id=$1 AND a.artifact_id::text=q.value
UNION ALL SELECT q.kind,q.value,'{}'::jsonb,NULL::jsonb,r.created_at FROM refs q JOIN source_regions r ON q.kind='source_region' AND r.subject_id=$1 AND r.source_region_id::text=q.value
"#).bind(subject.0).bind(kinds).bind(values).fetch_all(&mut **tx).await.map_err(db)?;
    let keys = [
        "input_access",
        "source_media_type",
        "source_duration_seconds",
        "sampled_duration_seconds",
        "requested_timestamps",
        "omitted_sample_timestamps",
        "sampled_timestamps",
        "frame_coverage",
        "partial_coverage",
        "truncated",
        "video_degradation",
        "sampling_policy",
        "animated_timeline",
    ];
    rows.into_iter()
        .map(|row| {
            let reference = parse_reference(
                &row.try_get::<String, _>("kind").map_err(db)?,
                &row.try_get::<String, _>("value").map_err(db)?,
            )?;
            let quality: serde_json::Value = row.try_get("quality").map_err(db)?;
            let mut visible = serde_json::Map::new();
            visible.insert(
                "formed_stamp".into(),
                row.try_get::<Option<serde_json::Value>, _>("formed_stamp")
                    .map_err(db)?
                    .unwrap_or(serde_json::Value::Null),
            );
            visible.insert(
                "recorded_at".into(),
                serde_json::to_value(row.try_get::<DateTime<Utc>, _>("created_at").map_err(db)?)
                    .map_err(|e| Error::Infrastructure(e.to_string()))?,
            );
            for key in keys {
                if let Some(value) = quality.get(key) {
                    visible.insert(key.into(), value.clone());
                }
            }
            Ok((reference, serde_json::Value::Object(visible)))
        })
        .collect()
}
