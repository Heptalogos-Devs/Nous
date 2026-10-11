// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::material_service_server::MaterialService {
        forward {
            get_occurrence(p::ObjectRequest) -> p::Occurrence;
            get_source_region(p::ObjectRequest) -> p::SourceRegion;
            get_derived_representation(p::ObjectRequest) -> p::DerivedRepresentation;
            get_artifact(p::ObjectRequest) -> p::Artifact;
            list_artifacts(p::ListRequest) -> p::ListArtifactsResponse;
            materialize_evidence(p::MaterializeRequest) -> p::MaterializedEvidence;
        }
        custom {
            async fn list_occurrences(
                &self,
                request: Request<p::OccurrenceListRequest>,
            ) -> std::result::Result<Response<p::OccurrenceListResponse>, Status> {
                let input = request.into_inner();
                let result: nous_core::Result<_> = async {
                    let (ids, truncated) = self.0.material.occurrence_ids_for_artifact(
                        SubjectId(id(&input.subject_id)?), nous_core::ArtifactId(id(&input.artifact_id)?), input.limit,
                    ).await?;
                    let mut items = Vec::with_capacity(ids.len());
                    for occurrence in ids {
                        items.push(self.get_occurrence(p::ObjectRequest {
                            subject_id: input.subject_id.clone(), id: occurrence.0.to_string(),
                        }).await?);
                    }
                    Ok(p::OccurrenceListResponse { items, truncated })
                }.await;
                result.map(Response::new).map_err(status)
            }
            async fn get_derived_region(
                &self,
                request: Request<p::ObjectRequest>,
            ) -> std::result::Result<Response<p::DerivedRegion>, Status> {
                let input = request.into_inner();
                let result: nous_core::Result<_> = async {
                    use sqlx::Row;
                    let r = sqlx::query(
                        "SELECT * FROM derived_regions WHERE subject_id=$1 AND derived_region_id=$2",
                    )
                    .bind(id(&input.subject_id)?)
                    .bind(id(&input.id)?)
                    .fetch_optional(self.0.store.pool())
                    .await
                    .map_err(nous_persistence::database_error)?
                    .ok_or_else(|| Error::new(nous_core::ErrorOwner::Material, nous_core::ErrorCode::NotFound, "derived region not found"))?;
                    Ok(p::DerivedRegion {
                        derived_region_id: input.id,
                        subject_id: input.subject_id,
                        representation_id: r
                            .try_get::<uuid::Uuid, _>("derived_representation_id")
                            .map_err(nous_persistence::database_error)?
                            .to_string(),
                        coordinate_kind: r
                            .try_get("coordinate_kind")
                            .map_err(nous_persistence::database_error)?,
                        coordinate: to_object(
                            r.try_get("coordinate")
                                .map_err(nous_persistence::database_error)?,
                        ),
                        coordinate_hash: r
                            .try_get("coordinate_hash")
                            .map_err(nous_persistence::database_error)?,
                        parent_derived_region_id: r
                            .try_get::<Option<uuid::Uuid>, _>("parent_derived_region_id")
                            .map_err(nous_persistence::database_error)?
                            .map(|id| id.to_string()),
                    })
                }
                .await;
                result.map(Response::new).map_err(status)
            }
            async fn get_producer(
                &self,
                request: Request<p::ObjectRequest>,
            ) -> std::result::Result<Response<p::ProducerSignature>, Status> {
                let input = request.into_inner();
                let result: nous_core::Result<_> = async {
                    use sqlx::Row;
                    let subject = SubjectId(id(&input.subject_id)?);
                    let producer = id(&input.id)?;
                    let referenced = self.0.material.references_producer(subject, producer).await?
                        || match &self.0.memory {
                            Some(memory) => memory.references_producer(subject, producer).await?,
                            None => false,
                        };
                    if !referenced {
                        return Err(Error::new(nous_core::ErrorOwner::Material, nous_core::ErrorCode::NotFound, "producer is not referenced by Subject"));
                    }
                    let r = sqlx::query("SELECT * FROM producer_signatures WHERE producer_signature_id=$1")
                        .bind(producer).fetch_optional(self.0.store.pool()).await
                        .map_err(nous_persistence::database_error)?
                        .ok_or_else(|| Error::new(nous_core::ErrorOwner::Material, nous_core::ErrorCode::NotFound, "producer signature not found"))?;
                    Ok(p::ProducerSignature {
            model_role: r.try_get("model_role").map_err(nous_persistence::database_error)?,
            model_profile: r.try_get("model_profile").map_err(nous_persistence::database_error)?,
            execution_profile: r.try_get("execution_profile").map_err(nous_persistence::database_error)?,
            inference_controls_digest: r.try_get("inference_controls_digest").map_err(nous_persistence::database_error)?,
            role_policy_digest: r.try_get("role_policy_digest").map_err(nous_persistence::database_error)?,
            prompt_id: r.try_get("prompt_id").map_err(nous_persistence::database_error)?,
            prompt_digest: r.try_get("prompt_digest").map_err(nous_persistence::database_error)?,
 signature_hash: r.try_get("signature_hash").map_err(nous_persistence::database_error)?, provider_class: r.try_get("provider_class").map_err(nous_persistence::database_error)?, operation: r.try_get::<String,_>("operation").map_err(nous_persistence::database_error)?.replace('.',"_"), implementation: r.try_get("implementation").map_err(nous_persistence::database_error)?, model_identity: r.try_get("model_identity").map_err(nous_persistence::database_error)?, model_revision: r.try_get("model_revision").map_err(nous_persistence::database_error)?, output_schema_digest: r.try_get("output_schema_digest").map_err(nous_persistence::database_error)?, preprocessing_identity: r.try_get("preprocessing_identity").map_err(nous_persistence::database_error)?, preprocessing_revision: r.try_get("preprocessing_revision").map_err(nous_persistence::database_error)?, config_digest: r.try_get("config_digest").map_err(nous_persistence::database_error)? })
                }.await;
                result.map(Response::new).map_err(status)
            }
            async fn list_derived_representations(
                &self,
                request: Request<p::RepresentationListRequest>,
            ) -> std::result::Result<Response<p::RepresentationListResponse>, Status> {
                let input = request.into_inner();
                let result: nous_core::Result<_> = async {
                    if input.limit == 0 || input.limit > 200 { return Err(Error::new(nous_core::ErrorOwner::Material, nous_core::ErrorCode::InvalidRequest, "representation limit must be 1..200")); }
                    let subject = SubjectId(id(&input.subject_id)?);
                    self.0.store.require_subject(subject).await?;
                    let source = input.source_region_id.as_deref().map(id).transpose()?;
                    let artifact = input.artifact_id.as_deref().map(id).transpose()?;
                    let ids: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT d.derived_representation_id FROM derived_representations d WHERE d.subject_id=$1 AND ($2::uuid IS NULL OR EXISTS(SELECT 1 FROM representation_source_regions($1,d.derived_representation_id) roots WHERE roots.source_region_id=$2)) AND ($3::uuid IS NULL OR EXISTS(SELECT 1 FROM representation_source_regions($1,d.derived_representation_id) roots JOIN source_regions s USING(source_region_id) WHERE s.artifact_id=$3)) AND ($4::text IS NULL OR d.representation_kind=$4) AND NOT EXISTS(SELECT 1 FROM derived_representations newer WHERE newer.supersedes=d.derived_representation_id) ORDER BY d.derived_representation_id DESC LIMIT $5")
                        .bind(subject.0).bind(source).bind(artifact).bind(input.kind).bind(i64::from(input.limit)+1).fetch_all(self.0.store.pool()).await.map_err(nous_persistence::database_error)?;
                    let truncated = ids.len()>input.limit as usize;
                    let mut items = Vec::new();
                    for value in ids.into_iter().take(input.limit as usize) { items.push(self.get_derived_representation(p::ObjectRequest { subject_id: input.subject_id.clone(), id: value.to_string() }).await?); }
                    Ok(p::RepresentationListResponse { items, truncated })
                }.await;
                result.map(Response::new).map_err(status)
            }
            async fn get_limits(
                &self,
                _: Request<()>,
            ) -> std::result::Result<Response<p::MaterialLimits>, Status> {
                Ok(Response::new(p::MaterialLimits {
                    max_upload_bytes: self.0.material.max_upload_bytes,
                }))
            }
        }
    }
}
