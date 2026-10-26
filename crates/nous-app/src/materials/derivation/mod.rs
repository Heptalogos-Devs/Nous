mod adopt;
mod input;
pub(crate) mod model;
mod output;
mod preparation;
mod worker;

use crate::App;
use nous_domain::{media::*, *};
use nous_storage::{BeginOperation, OperationReceipt, operation_key};
use serde_json::json;

impl App {
    pub async fn derive_media(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: DeriveMediaRequest,
    ) -> Result<OperationReceipt> {
        request.validate()?;
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&namespace, &request.subject_id)?;
        let source = self
            .sources
            .resolve(ctx, &namespace, &request.subject_id, &request.source)
            .await?;
        if request.strategy != DerivationStrategy::DescriptionOnly
            && !matches!(source.payload, RepresentationPayload::Artifact { .. })
        {
            return Err(Error::invalid(
                "media structuring requires original media or an exact prepared artifact",
            ));
        }
        let key = operation_key(
            ctx,
            "media.derive",
            &format!("namespace:{namespace}/subject:{}", request.subject_id),
            operation_id,
            &serde_json::to_value(&request)?,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let observation = tx.observation(&namespace, &request.observation_id).await?;
        if observation.subject_id != request.subject_id
            || !source.roots.contains(&observation.material_id)
        {
            return Err(Error::invalid(
                "source event does not belong to this subject and media",
            ));
        }
        let result_target = if let Some(previous) = &request.revise {
            let head = tx
                .representation_head(&namespace, &request.subject_id, &previous.id)
                .await?
                .ok_or_else(|| Error::new(ErrorCode::NotFound, "representation not found"))?;
            if head.reference() != *previous {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "representation head changed",
                ));
            }
            let same_purpose = matches!(
                (request.strategy, &head.payload),
                (
                    DerivationStrategy::DescriptionOnly,
                    RepresentationPayload::Text { .. }
                ) | (
                    DerivationStrategy::DirectStructured
                        | DerivationStrategy::DescribeThenStructure,
                    RepresentationPayload::Interpretation { .. }
                )
            );
            if !same_purpose {
                return Err(Error::invalid(
                    "revision must preserve the representation's purpose",
                ));
            }
            RepresentationRef {
                id: previous.id.clone(),
                revision: previous
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| Error::invalid("representation revision overflow"))?,
            }
        } else {
            RepresentationRef {
                id: DerivedRepresentationId::new(),
                revision: 1,
            }
        };
        let description_target = match request.strategy {
            DerivationStrategy::DescriptionOnly => Some(result_target.clone()),
            DerivationStrategy::DescribeThenStructure => Some(RepresentationRef {
                id: DerivedRepresentationId::new(),
                revision: 1,
            }),
            DerivationStrategy::DirectStructured => None,
        };
        let job = self.tasks.new_job(
            &tx,
            "media.derive",
            Some(namespace),
            Some(request.subject_id.clone()),
        );
        let run = MediaDerivationRun {
            job,
            request,
            source_event_kind: observation.kind,
            description_target,
            result_target,
            prepared: None,
            description: None,
            result: None,
        };
        self.tasks.submit_in(&mut tx, &run.job).await?;
        tx.insert_media_derivation(&run).await?;
        tx.commit(json!(run)).await
    }

    pub async fn media_derivation(
        &self,
        ctx: &CallContext,
        job_id: &str,
    ) -> Result<MediaDerivationRun> {
        let run = self.database.media_derivation(job_id).await?;
        let (namespace, subject) = run.job.subject_scope()?;
        ctx.require_subject(namespace, subject)?;
        Ok(run)
    }

    pub async fn media_representation(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        reference: &RepresentationRef,
    ) -> Result<DerivedRepresentation> {
        let namespace = self.database.subject_namespace(subject).await?;
        self.sources
            .resolve(
                ctx,
                &namespace,
                subject,
                &MediaReference::Representation {
                    reference: reference.clone(),
                },
            )
            .await?;
        self.database
            .representation(&namespace, subject, reference)
            .await
    }

    pub async fn media_representation_history(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        id: &DerivedRepresentationId,
        limit: usize,
    ) -> Result<Vec<DerivedRepresentation>> {
        let namespace = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&namespace, subject)?;
        let history = self
            .database
            .representation_history(&namespace, subject, id, limit)
            .await?;
        for value in &history {
            self.sources
                .resolve(
                    ctx,
                    &namespace,
                    subject,
                    &MediaReference::Representation {
                        reference: value.reference(),
                    },
                )
                .await?;
        }
        Ok(history)
    }

    pub async fn create_source_region(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: SourceRegionRequest,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        self.sources
            .resolve(
                ctx,
                &namespace,
                &request.subject_id,
                &MediaReference::Original {
                    material_id: request.material_id.clone(),
                },
            )
            .await?;
        let key = operation_key(
            ctx,
            "media.region",
            &format!("namespace:{namespace}"),
            operation_id,
            &serde_json::to_value(&request)?,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let region = SourceRegion {
            id: SourceRegionId::new(),
            namespace_id: namespace,
            material_id: request.material_id,
            parent: request.parent,
            coordinate: request.coordinate,
            coordinate_hash: String::new(),
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        let region = tx.put_source_region(region).await?;
        tx.commit(json!(region)).await
    }

    pub async fn source_region(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        id: &SourceRegionId,
    ) -> Result<SourceRegion> {
        let namespace = self.database.subject_namespace(subject).await?;
        self.sources
            .resolve(
                ctx,
                &namespace,
                subject,
                &MediaReference::SourceRegion { id: id.clone() },
            )
            .await?;
        self.database.source_region(&namespace, id).await
    }

    pub async fn create_derived_region(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: DerivedRegionRequest,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        self.sources
            .resolve(
                ctx,
                &namespace,
                &request.subject_id,
                &MediaReference::Representation {
                    reference: request.representation.clone(),
                },
            )
            .await?;
        let key = operation_key(
            ctx,
            "media.derived_region",
            &format!("namespace:{namespace}/subject:{}", request.subject_id),
            operation_id,
            &serde_json::to_value(&request)?,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let region = DerivedRegion {
            id: DerivedRegionId::new(),
            namespace_id: namespace,
            subject_id: request.subject_id,
            representation: request.representation,
            parent: request.parent,
            selector: request.selector,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        let region = tx.put_derived_region(region).await?;
        tx.commit(json!(region)).await
    }

    pub async fn derived_region(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        id: &DerivedRegionId,
    ) -> Result<DerivedRegion> {
        let namespace = self.database.subject_namespace(subject).await?;
        self.sources
            .resolve(
                ctx,
                &namespace,
                subject,
                &MediaReference::DerivedRegion { id: id.clone() },
            )
            .await?;
        self.database.derived_region(&namespace, subject, id).await
    }
}
