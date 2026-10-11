// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;

fn temporal_columns(
    value: &nous_core::TemporalExtent,
) -> (
    &'static str,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
) {
    match value {
        nous_core::TemporalExtent::Unknown => ("unknown", None, None),
        nous_core::TemporalExtent::Instant { at } => ("instant", Some(*at), None),
        nous_core::TemporalExtent::Interval { start, end } => ("interval", *start, *end),
    }
}

impl MaterialService {
    /// An Artifact may have multiple independent observations; never choose one implicitly.
    pub async fn occurrence_ids_for_artifact(
        &self,
        subject: SubjectId,
        artifact: ArtifactId,
        limit: u32,
    ) -> Result<(Vec<OccurrenceId>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("occurrence limit must be 1..200".into()));
        }
        self.store
            .validate_reference(subject, &CognitiveRef::Artifact(artifact))
            .await?;
        let mut ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT occurrence_id FROM observation_occurrences WHERE subject_id=$1 AND artifact_id=$2 ORDER BY observed_at DESC,occurrence_id DESC LIMIT $3",
        )
        .bind(subject.0).bind(artifact.0).bind(i64::from(limit)+1)
        .fetch_all(self.store.pool()).await.map_err(db)?;
        let truncated = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        Ok((ids.into_iter().map(OccurrenceId).collect(), truncated))
    }

    pub async fn record_observation(&self, input: ObservationInput) -> Result<AcceptedObservation> {
        self.record_observation_once(input, None).await
    }

    // Observation admission keeps Authority, CAS and runtime ordering explicit.
    #[expect(
        clippy::too_many_lines,
        reason = "observation admission owns one transaction and its runtime handoff"
    )]
    pub async fn record_observation_once(
        &self,
        input: ObservationInput,
        request_id: Option<Uuid>,
    ) -> Result<AcceptedObservation> {
        self.store.require_subject(input.subject).await?;
        if let Some(session) = input.session {
            self.cognition
                .require_session(input.subject, session)
                .await?;
        }
        let mut tx = self.store.begin().await?;
        let request_digest = blake3::hash(
            &serde_json::to_vec(&input).map_err(|error| Error::Invalid(error.to_string()))?,
        )
        .to_hex()
        .to_string();
        if let Some(request_id) = request_id {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(request_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            if let Some(row) = sqlx::query("SELECT subject_id,request_digest,accepted FROM observation_request_bindings WHERE request_id=$1")
                .bind(request_id).fetch_optional(&mut *tx).await.map_err(db)? {
                if row.try_get::<Uuid,_>("subject_id").map_err(db)? != input.subject.0
                    || row.try_get::<String,_>("request_digest").map_err(db)? != request_digest {
                    return Err(nous_core::DomainError::new(nous_core::DomainErrorCode::OperationIdConflict, "Observation request_id already binds different input")
                        .with_context("request_id", request_id).into());
                }
                let accepted: AcceptedObservation = serde_json::from_value(row.try_get("accepted").map_err(db)?)
                    .map_err(|error| Error::Infrastructure(error.to_string()))?;
                tx.commit().await.map_err(db)?;
                self.admit_observation(&input, accepted.occurrence.occurrence_id).await?;
                return Ok(accepted);
            }
        }
        let now = self.cognition.now(input.subject);
        let object_guard = if matches!(
            &input.material,
            ObservationMaterial::InlineText { .. } | ObservationMaterial::StructuredJson { .. }
        ) {
            Some(self.objects.reference_guard(false).await?)
        } else {
            None
        };
        let (artifact, _) = self
            .material_to_artifact(input.subject, &input.material, now)
            .await?;
        if let Some(artifact) = &artifact {
            artifact.validate()?;
        }
        let occurrence_id = OccurrenceId::new();
        let mut occurrence = ObservationOccurrence {
            occurrence_id,
            subject_id: input.subject,
            artifact_id: artifact.as_ref().map(|artifact| artifact.artifact_id),
            source_class: input.occurrence.source_class.clone(),
            external_object_ref: input.occurrence.external_object_ref.clone().or_else(|| {
                match &input.material {
                    ObservationMaterial::ExternalObjectRef { object_ref } => {
                        Some(object_ref.clone())
                    }
                    _ => None,
                }
            }),
            occurred_time: input.occurrence.occurred_time.clone(),
            observed_at: input.occurrence.observed_at.unwrap_or(now),
            conversation_ref: input.occurrence.conversation_ref.clone(),
            actor_entity_ref: input.occurrence.actor_entity_ref.clone(),
            context: input.occurrence.context.clone(),
            created_at: now,
        };
        occurrence.validate()?;
        if let Some(entity) = &occurrence.actor_entity_ref {
            EntityRef::new(entity.as_str())?;
        }
        if let Some(object) = &occurrence.external_object_ref {
            ObjectRef::new(object.as_str())?;
        }
        let artifact = if let Some(mut artifact) = artifact {
            let existing = sqlx::query("SELECT artifact_id,byte_length,storage_key,created_at,metadata FROM artifacts WHERE subject_id=$1 AND content_hash=$2")
                .bind(input.subject.0).bind(&artifact.content_hash).fetch_optional(&mut *tx).await.map_err(db)?;
            if let Some(row) = existing {
                artifact.artifact_id = ArtifactId(row.try_get("artifact_id").map_err(db)?);
                artifact.byte_length = row.try_get::<i64, _>("byte_length").map_err(db)? as u64;
                artifact.storage_key = row.try_get("storage_key").map_err(db)?;
                artifact.created_at = row.try_get("created_at").map_err(db)?;
                artifact.metadata = row.try_get("metadata").map_err(db)?;
            } else {
                sqlx::query("INSERT INTO artifacts(artifact_id,subject_id,content_hash,byte_length,media_type,storage_key,created_at,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
                    .bind(artifact.artifact_id.0).bind(artifact.subject_id.0).bind(&artifact.content_hash).bind(artifact.byte_length as i64).bind(&artifact.media_type).bind(&artifact.storage_key).bind(artifact.created_at).bind(&artifact.metadata).execute(&mut *tx).await.map_err(db)?;
            }
            Some(artifact)
        } else {
            None
        };
        occurrence.artifact_id = artifact.as_ref().map(|artifact| artifact.artifact_id);
        let (occurred_kind, occurred_start, occurred_end) =
            temporal_columns(&occurrence.occurred_time);
        sqlx::query("INSERT INTO observation_occurrences(occurrence_id,subject_id,artifact_id,source_class,external_object_ref,occurred_time_kind,occurred_time_start,occurred_time_end,observed_at,conversation_ref,actor_entity_ref,context,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
            .bind(occurrence.occurrence_id.0).bind(occurrence.subject_id.0).bind(occurrence.artifact_id.map(|id| id.0)).bind(occurrence.source_class.as_str()).bind(occurrence.external_object_ref.as_ref().map(ObjectRef::as_str)).bind(occurred_kind).bind(occurred_start).bind(occurred_end).bind(occurrence.observed_at).bind(&occurrence.conversation_ref).bind(occurrence.actor_entity_ref.as_ref().map(EntityRef::as_str)).bind(&occurrence.context).bind(occurrence.created_at).execute(&mut *tx).await.map_err(db)?;
        let source_region = if let Some(artifact) = &artifact {
            let coordinate = serde_json::json!({});
            let coordinate_hash = blake3::hash(
                serde_json::to_string(&coordinate)
                    .unwrap_or_default()
                    .as_bytes(),
            )
            .to_hex()
            .to_string();
            let existing = sqlx::query("SELECT source_region_id,created_at FROM source_regions WHERE subject_id=$1 AND artifact_id=$2 AND coordinate_kind='whole_artifact' AND coordinate_hash=$3")
                .bind(input.subject.0).bind(artifact.artifact_id.0).bind(&coordinate_hash).fetch_optional(&mut *tx).await.map_err(db)?;
            let (id, region_created_at) = if let Some(row) = existing {
                (
                    SourceRegionId(row.try_get("source_region_id").map_err(db)?),
                    row.try_get("created_at").map_err(db)?,
                )
            } else {
                let id = SourceRegionId::new();
                sqlx::query("INSERT INTO source_regions(source_region_id,subject_id,artifact_id,coordinate_kind,coordinate,coordinate_hash,created_at) VALUES($1,$2,$3,'whole_artifact',$4,$5,$6)")
                    .bind(id.0).bind(input.subject.0).bind(artifact.artifact_id.0).bind(&coordinate).bind(&coordinate_hash).bind(now).execute(&mut *tx).await.map_err(db)?;
                (id, now)
            };
            let region = SourceRegion {
                source_region_id: id,
                subject_id: input.subject,
                artifact_id: artifact.artifact_id,
                coordinate_kind: "whole_artifact".into(),
                coordinate,
                coordinate_hash,
                parent_source_region_id: None,
                created_at: region_created_at,
            };
            region.validate()?;
            Some(region)
        } else {
            None
        };
        let mut mention_ids = Vec::new();
        for mention in &input.entities {
            if mention.surface.trim().is_empty() {
                return Err(Error::Invalid("entity mention surface is required".into()));
            }
            if let Some(entity) = &mention.entity_ref {
                EntityRef::new(entity.as_str())?;
            }
            let mention_id = Uuid::now_v7();
            mention_ids.push(mention_id);
            sqlx::query("INSERT INTO entity_mentions(mention_id,subject_id,occurrence_id,surface,semantic_role,created_at) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(mention_id).bind(input.subject.0).bind(occurrence_id.0).bind(&mention.surface).bind(&mention.semantic_role).bind(now).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("INSERT INTO entity_binding_revisions(binding_revision_id,mention_id,revision_no,entity_ref,binding_state,created_at) VALUES($1,$2,1,$3,$4,$5)")
                .bind(Uuid::now_v7()).bind(mention_id).bind(mention.entity_ref.as_ref().map(EntityRef::as_str)).bind(if mention.entity_ref.is_some(){"bound"}else{"unbound"}).bind(now).execute(&mut *tx).await.map_err(db)?;
        }
        if let Some(region) = &source_region {
            sqlx::query("INSERT INTO coverage_needs(coverage_need_id,subject_id,source_region_id,representation_kind,capability_operation,requirement,state,updated_at) VALUES($1,$2,$3,'extracted_text','document_extraction','preferred',$4,$5) ON CONFLICT(subject_id,source_region_id,representation_kind,capability_operation) DO UPDATE SET updated_at=excluded.updated_at")
                .bind(Uuid::now_v7()).bind(input.subject.0).bind(region.source_region_id.0).bind(if matches!(input.material, ObservationMaterial::InlineText{..}){"ready"}else{"missing"}).bind(now).execute(&mut *tx).await.map_err(db)?;
        }
        let recorded_seq = nous_persistence::AuthorityStore::invalidate_in(
            &mut tx,
            input.subject,
            ProjectionInvalidation::text(),
        )
        .await?;
        if let Some(session) = input.session {
            if occurrence.observed_at > now + chrono::Duration::minutes(5) {
                return Err(Error::Invalid(
                    "observed_at is too far in the future".into(),
                ));
            }
            self.cognition
                .capture_experience_in(
                    &mut tx,
                    nous_runtime::ExperienceInput {
                        subject: input.subject,
                        recorded_seq,
                        occurrence: occurrence_id,
                        session,
                        observed_at: occurrence.observed_at,
                        source_class: occurrence.source_class.as_str().into(),
                        conversation_ref: occurrence.conversation_ref.clone(),
                        actor_entity_ref: occurrence
                            .actor_entity_ref
                            .as_ref()
                            .map(|value| value.as_str().into()),
                    },
                )
                .await?;
        }
        let accepted = AcceptedObservation {
            artifact,
            occurrence,
            source_region,
            mention_ids,
            resident: input.session.is_some() && input.runtime.admit,
            memory_revisions: Vec::new(),
        };
        let mut addresses = vec![CognitiveRef::Occurrence(accepted.occurrence.occurrence_id)];
        if let Some(artifact) = &accepted.artifact {
            addresses.push(CognitiveRef::Artifact(artifact.artifact_id));
        }
        if let Some(region) = &accepted.source_region {
            addresses.push(CognitiveRef::SourceRegion(region.source_region_id));
        }
        if let Some(entity) = &accepted.occurrence.actor_entity_ref {
            addresses.push(CognitiveRef::Entity(entity.clone()));
        }
        addresses.extend(
            input
                .entities
                .iter()
                .filter_map(|mention| mention.entity_ref.clone())
                .map(CognitiveRef::Entity),
        );
        self.store
            .ensure_identity_addresses_in(&mut tx, input.subject, &addresses, "")
            .await?;
        if let Some(request_id) = request_id {
            sqlx::query("INSERT INTO observation_request_bindings(request_id,subject_id,request_digest,occurrence_id,accepted) VALUES($1,$2,$3,$4,$5)")
                .bind(request_id).bind(input.subject.0).bind(request_digest).bind(occurrence_id.0)
                .bind(serde_json::to_value(&accepted).map_err(|error| Error::Infrastructure(error.to_string()))?)
                .execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        drop(object_guard);
        self.admit_observation(&input, occurrence_id).await?;
        Ok(accepted)
    }

    async fn admit_observation(
        &self,
        input: &ObservationInput,
        occurrence_id: OccurrenceId,
    ) -> Result<()> {
        if let Some(session) = input.session.filter(|_| input.runtime.admit) {
            let mut admissions = vec![ResidentAdmission {
                reference: CognitiveRef::Occurrence(occurrence_id),
                reason: "observed".into(),
                hold_until: input.runtime.hold_until,
                state: ResidentState::Resident,
            }];
            for mention in &input.entities {
                if let Some(entity) = &mention.entity_ref {
                    admissions.push(ResidentAdmission {
                        reference: CognitiveRef::Entity(entity.clone()),
                        reason: "observed".into(),
                        hold_until: input.runtime.hold_until,
                        state: ResidentState::Resident,
                    });
                }
            }
            if let ObservationMaterial::ResourceAvailability { resource } = &input.material {
                ResourceRef::new(resource.as_str())?;
                admissions.push(ResidentAdmission {
                    reference: CognitiveRef::Resource(resource.clone()),
                    reason: "resource_awareness".into(),
                    hold_until: input.runtime.hold_until,
                    state: ResidentState::Resident,
                });
            }
            self.cognition
                .admit_batch(input.subject, session, admissions)
                .await?;
        }
        Ok(())
    }

    async fn material_to_artifact(
        &self,
        subject: SubjectId,
        material: &ObservationMaterial,
        now: DateTime<Utc>,
    ) -> Result<(Option<Artifact>, Option<Vec<u8>>)> {
        match material {
            ObservationMaterial::InlineText { text, media_type } => {
                let bytes = text.as_bytes().to_vec();
                self.bytes_to_artifact(subject, bytes, media_type, serde_json::json!({}), now)
                    .await
            }
            ObservationMaterial::StructuredJson { value } => {
                let bytes =
                    serde_json::to_vec(value).map_err(|error| Error::Invalid(error.to_string()))?;
                self.bytes_to_artifact(
                    subject,
                    bytes,
                    "application/json",
                    serde_json::json!({}),
                    now,
                )
                .await
            }
            ObservationMaterial::ArtifactRef { artifact_id } => {
                let artifact = self.artifact(subject, *artifact_id).await?;
                Ok((Some(artifact), None))
            }
            ObservationMaterial::ExternalObjectRef { .. }
            | ObservationMaterial::ResourceAvailability { .. } => Ok((None, None)),
        }
    }

    async fn bytes_to_artifact(
        &self,
        subject: SubjectId,
        bytes: Vec<u8>,
        media_type: &str,
        metadata: serde_json::Value,
        now: DateTime<Utc>,
    ) -> Result<(Option<Artifact>, Option<Vec<u8>>)> {
        if bytes.len() as u64 > self.max_upload_bytes {
            return Err(Error::Invalid(
                "material exceeds configured byte bound".into(),
            ));
        }
        let (hash, size) = self
            .objects
            .put_chunks(
                futures::stream::iter([Ok::<Vec<u8>, Error>(bytes.clone())]),
                self.max_upload_bytes,
            )
            .await?;
        let artifact = Artifact {
            artifact_id: ArtifactId::new(),
            subject_id: subject,
            content_hash: hash.clone(),
            byte_length: size,
            media_type: media_type.to_owned(),
            storage_key: hash,
            created_at: now,
            metadata,
        };
        Ok((Some(artifact), Some(bytes)))
    }

    pub async fn upload_stream<S>(
        &self,
        subject: SubjectId,
        metadata: UploadMetadata,
        chunks: S,
    ) -> Result<Artifact>
    where
        S: Stream<Item = Result<Vec<u8>>> + Send,
    {
        self.store.require_subject(subject).await?;
        let guard = self.objects.reference_guard(false).await?;
        let (hash, size) = self
            .objects
            .put_chunks(chunks, self.max_upload_bytes)
            .await?;
        let now = self.cognition.now(subject);
        if !self.store.subject_exists(subject).await? {
            drop(guard);
            return Err(Error::NotFound("subject not found".into()));
        }
        let proposed_artifact_id = ArtifactId::new();
        let mut tx = self.store.begin().await?;
        sqlx::query("INSERT INTO artifacts(artifact_id,subject_id,content_hash,byte_length,media_type,storage_key,created_at,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(subject_id,content_hash) DO NOTHING")
            .bind(proposed_artifact_id.0).bind(subject.0).bind(&hash).bind(size as i64).bind(&metadata.media_type).bind(&hash).bind(now).bind(&metadata.metadata)
            .execute(&mut *tx).await.map_err(db)?;
        let artifact_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT artifact_id FROM artifacts WHERE subject_id=$1 AND content_hash=$2",
        )
        .bind(subject.0)
        .bind(&hash)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        self.store
            .ensure_identity_addresses_in(
                &mut tx,
                subject,
                &[CognitiveRef::Artifact(ArtifactId(artifact_id))],
                "",
            )
            .await?;
        tx.commit().await.map_err(db)?;
        let artifact = self.artifact(subject, ArtifactId(artifact_id)).await?;
        drop(guard);
        Ok(artifact)
    }

    pub async fn artifact(&self, subject: SubjectId, artifact: ArtifactId) -> Result<Artifact> {
        let row = sqlx::query("SELECT artifact_id,subject_id,content_hash,byte_length,media_type,storage_key,created_at,metadata FROM artifacts WHERE subject_id=$1 AND artifact_id=$2")
            .bind(subject.0).bind(artifact.0).fetch_optional(self.store.pool()).await.map_err(db)?.ok_or_else(||Error::NotFound("artifact not found".into()))?;
        Ok(Artifact {
            artifact_id: ArtifactId(row.try_get("artifact_id").map_err(db)?),
            subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
            content_hash: row.try_get("content_hash").map_err(db)?,
            byte_length: row.try_get::<i64, _>("byte_length").map_err(db)? as u64,
            media_type: row.try_get("media_type").map_err(db)?,
            storage_key: row.try_get("storage_key").map_err(db)?,
            created_at: row.try_get("created_at").map_err(db)?,
            metadata: row.try_get("metadata").map_err(db)?,
        })
    }

    pub async fn artifact_stream(
        &self,
        subject: SubjectId,
        artifact: ArtifactId,
    ) -> Result<(Artifact, impl Stream<Item = Result<Vec<u8>>> + Send + use<>)> {
        let artifact = self.artifact(subject, artifact).await?;
        let stream = self.objects.stream(&artifact.content_hash).await?;
        Ok((artifact, stream))
    }
}
