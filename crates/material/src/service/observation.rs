// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::artifact::decode_artifact;
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
                return Ok(accepted);
            }
        }
        let observed_stamp = self
            .cognition
            .capture_semantic_in(&mut tx, input.subject)
            .await?;
        let now = observed_stamp.recorded_at;
        let object_guard = if matches!(
            &input.material,
            ObservationMaterial::InlineText { .. }
                | ObservationMaterial::StructuredJson { .. }
                | ObservationMaterial::ArtifactRef { .. }
        ) {
            Some(self.objects.reference_guard(false).await?)
        } else {
            None
        };
        let artifact = self
            .material_to_artifact(&mut tx, input.subject, &input.material, now)
            .await?;
        let occurrence_id = OccurrenceId::new();
        let occurrence = ObservationOccurrence {
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
            occurred_time_domain_ref: input.occurrence.occurred_time_domain_ref.clone(),
            observed_stamp: input.occurrence.observed_at.map_or_else(
                || observed_stamp.clone(),
                |at| nous_core::DomainStamp {
                    time_domain_ref: "time:real".into(),
                    at,
                    recorded_at: now,
                },
            ),
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
        let (occurred_kind, occurred_start, occurred_end) =
            temporal_columns(&occurrence.occurred_time);
        sqlx::query("INSERT INTO observation_occurrences(occurrence_id,subject_id,artifact_id,source_class,external_object_ref,occurred_time_kind,occurred_time_start,occurred_time_end,observed_at,conversation_ref,actor_entity_ref,context,created_at,occurred_time_domain_ref,observed_stamp) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
            .bind(occurrence.occurrence_id.0).bind(occurrence.subject_id.0).bind(occurrence.artifact_id.map(|id| id.0)).bind(occurrence.source_class.as_str()).bind(occurrence.external_object_ref.as_ref().map(ObjectRef::as_str)).bind(occurred_kind).bind(occurred_start).bind(occurred_end).bind(occurrence.observed_at).bind(&occurrence.conversation_ref).bind(occurrence.actor_entity_ref.as_ref().map(EntityRef::as_str)).bind(&occurrence.context).bind(occurrence.created_at).bind(&occurrence.occurred_time_domain_ref).bind(serde_json::to_value(&occurrence.observed_stamp).map_err(|e|Error::Infrastructure(e.to_string()))?).execute(&mut *tx).await.map_err(db)?;
        let source_region = if let Some(artifact) = &artifact {
            let coordinate = serde_json::json!({});
            let coordinate_hash = blake3::hash(b"{}").to_hex().to_string();
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
            AuthorityStore::add_model_result_dependencies_in(
                &mut tx,
                input.subject,
                "material",
                nous_core::OperationId(request_id),
                &addresses,
            )
            .await?;
            sqlx::query("INSERT INTO observation_request_bindings(request_id,subject_id,request_digest,occurrence_id,accepted) VALUES($1,$2,$3,$4,$5)")
                .bind(request_id).bind(input.subject.0).bind(request_digest).bind(occurrence_id.0)
                .bind(serde_json::to_value(&accepted).map_err(|error| Error::Infrastructure(error.to_string()))?)
                .execute(&mut *tx).await.map_err(db)?;
        }
        self.admit_observation_in(&mut tx, &input, occurrence_id)
            .await?;
        tx.commit().await.map_err(db)?;
        drop(object_guard);
        Ok(accepted)
    }

    async fn admit_observation_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &ObservationInput,
        occurrence_id: OccurrenceId,
    ) -> Result<()> {
        if let Some(session) = input.session.filter(|_| input.runtime.admit) {
            let mut admissions = vec![ResidentAdmission {
                last_meaningful_use_at: None,
                reference: CognitiveRef::Occurrence(occurrence_id),
                reason: "observed".into(),
                hold_until: input.runtime.hold_until,
                state: ResidentState::Resident,
            }];
            for mention in &input.entities {
                if let Some(entity) = &mention.entity_ref {
                    admissions.push(ResidentAdmission {
                        last_meaningful_use_at: None,
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
                    last_meaningful_use_at: None,
                    reference: CognitiveRef::Resource(resource.clone()),
                    reason: "resource_awareness".into(),
                    hold_until: input.runtime.hold_until,
                    state: ResidentState::Resident,
                });
            }
            self.cognition
                .admit_batch_in(tx, input.subject, session, admissions)
                .await?;
        }
        Ok(())
    }

    async fn material_to_artifact(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        material: &ObservationMaterial,
        now: DateTime<Utc>,
    ) -> Result<Option<Artifact>> {
        let (bytes, media_type) = match material {
            ObservationMaterial::InlineText { text, media_type } => {
                (text.as_bytes().to_vec(), media_type.as_str())
            }
            ObservationMaterial::StructuredJson { value } => (
                serde_json::to_vec(value).map_err(|error| Error::Invalid(error.to_string()))?,
                "application/json",
            ),
            ObservationMaterial::ArtifactRef { artifact_id } => {
                let row = sqlx::query("SELECT artifact_id,subject_id,content_hash,byte_length,media_type,created_at,metadata FROM artifacts WHERE subject_id=$1 AND artifact_id=$2")
                    .bind(subject.0).bind(artifact_id.0).fetch_optional(&mut **tx).await.map_err(db)?
                    .ok_or_else(|| Error::NotFound("artifact not found".into()))?;
                return decode_artifact(row).map(Some);
            }
            ObservationMaterial::ExternalObjectRef { .. }
            | ObservationMaterial::ResourceAvailability { .. } => return Ok(None),
        };
        ArtifactRegistration::new(self.store.clone(), self.objects.clone())
            .write_in(
                tx,
                subject,
                UploadMetadata {
                    media_type: media_type.into(),
                    metadata: serde_json::json!({}),
                },
                futures::stream::iter([Ok(bytes)]),
                self.max_upload_bytes,
                now,
            )
            .await
            .map(Some)
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
        let mut tx = self.store.begin().await?;
        let artifact = ArtifactRegistration::new(self.store.clone(), self.objects.clone())
            .write_in(
                &mut tx,
                subject,
                metadata,
                chunks,
                self.max_upload_bytes,
                self.cognition.now(subject),
            )
            .await?;
        tx.commit().await.map_err(db)?;
        drop(guard);
        Ok(artifact)
    }

    pub async fn artifact(&self, subject: SubjectId, artifact: ArtifactId) -> Result<Artifact> {
        let row = sqlx::query("SELECT artifact_id,subject_id,content_hash,byte_length,media_type,created_at,metadata FROM artifacts WHERE subject_id=$1 AND artifact_id=$2")
            .bind(subject.0).bind(artifact.0).fetch_optional(self.store.pool()).await.map_err(db)?.ok_or_else(||Error::NotFound("artifact not found".into()))?;
        decode_artifact(row)
    }

    pub async fn artifacts(
        &self,
        subject: SubjectId,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<Artifact>, bool)> {
        self.store.require_subject(subject).await?;
        let mut rows = sqlx::query("SELECT * FROM artifacts WHERE subject_id=$1 AND ($2::uuid IS NULL OR artifact_id>$2) ORDER BY artifact_id LIMIT $3")
            .bind(subject.0).bind(after).bind(limit+1).fetch_all(self.store.pool()).await.map_err(db)?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        Ok((
            rows.into_iter()
                .map(decode_artifact)
                .collect::<Result<_>>()?,
            more,
        ))
    }

    pub async fn artifact_stream(
        &self,
        subject: SubjectId,
        artifact: ArtifactId,
        start: Option<u64>,
        end: Option<u64>,
    ) -> Result<impl Stream<Item = Result<Vec<u8>>> + Send + use<>> {
        let artifact = self.artifact(subject, artifact).await?;
        let start = start.unwrap_or(0);
        let end = end.unwrap_or(artifact.byte_length);
        if start > end || end > artifact.byte_length {
            return Err(Error::Invalid("invalid byte range".into()));
        }
        let selected = (start != 0 || end != artifact.byte_length).then_some(start..end);
        self.objects.stream(&artifact.content_hash, selected).await
    }
}
