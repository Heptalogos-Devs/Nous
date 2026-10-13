// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::material_service_server::MaterialService {
        forward {
            read_lineage(p::MaterialLineageRequest) -> p::MaterialLineage;
            get_occurrence(p::ObjectRequest) -> p::Occurrence;
            get_source_region(p::ObjectRequest) -> p::SourceRegion;
            get_derived_representation(p::ObjectRequest) -> p::DerivedRepresentation;
            get_artifact(p::ObjectRequest) -> p::Artifact;
            list_artifacts(p::ListRequest) -> p::ListArtifactsResponse;
            materialize_evidence(p::MaterializeRequest) -> p::MaterializedEvidence;
        }
        custom {
            async fn clean(&self, _: Request<()>) -> std::result::Result<Response<p::MaterialCleanup>,Status> {
                self.0.material.clean().await.map(|(objects_deleted,staging_deleted)|Response::new(p::MaterialCleanup {objects_deleted,staging_deleted})).map_err(status)
            }
            async fn list_occurrences(
                &self,
                request: Request<p::OccurrenceListRequest>,
            ) -> std::result::Result<Response<p::OccurrenceListResponse>, Status> {
                let input = request.into_inner();
                let result: nous_core::Result<_> = async {
                    let (items, truncated) = self.0.material.occurrences_for_artifact(SubjectId(id(&input.subject_id)?), nous_core::ArtifactId(id(&input.artifact_id)?), input.limit).await?;
                    let items = items.into_iter().map(evidence::occurrence).collect();
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
                    let value = self.0.material.derived_region(SubjectId(id(&input.subject_id)?), nous_core::DerivedRegionId(id(&input.id)?)).await?;
                    Ok(evidence::derived_region(value))
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
                    let subject = SubjectId(id(&input.subject_id)?);
                    let producer = id(&input.id)?;
                    let referenced = self.0.material.references_producer(subject, producer).await?
                        || match &self.0.memory {
                            Some(memory) => memory.references_producer(subject, producer).await?,
                            None => false,
                        };
                    if !referenced {
                        return Err(Error::NotFound("producer is not referenced by Subject".into()));
                    }
                    Ok(evidence::producer(self.0.material.producer_signature(producer).await?))
                }.await;
                result.map(Response::new).map_err(status)
            }
            async fn list_derived_representations(
                &self,
                request: Request<p::RepresentationListRequest>,
            ) -> std::result::Result<Response<p::RepresentationListResponse>, Status> {
                let input = request.into_inner();
                let result: nous_core::Result<_> = async {
                    let (items, truncated) = self.0.material.derived_representations(SubjectId(id(&input.subject_id)?), input.source_region_id.as_deref().map(id).transpose()?, input.artifact_id.as_deref().map(id).transpose()?, input.kind, input.limit).await?;
                    let items = items.into_iter().map(evidence::representation).collect();
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
