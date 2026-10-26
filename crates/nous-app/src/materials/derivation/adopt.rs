use super::{input::MediaModelInput, preparation::digest};
use crate::App;
use nous_domain::{media::*, *};
use nous_foundation::models::InvocationMetadata;
use serde_json::json;

impl App {
    pub(super) async fn adopt_media_call(
        &self,
        run: &MediaDerivationRun,
        call: &ModelCall,
        input: &MediaModelInput,
        description: bool,
    ) -> Result<()> {
        let operation = if description {
            "media.description"
        } else {
            "media.interpretation"
        };
        let Some(mut tx) = self
            .worker_transaction(
                &run.job,
                operation,
                &run.job.id,
                &run.job.id,
                &json!({"call_id":call.id}),
            )
            .await?
        else {
            return Ok(());
        };
        let mut current = tx.media_derivation(&run.job.id).await?;
        if matches!(current.job.state, JobState::Succeeded | JobState::Cancelled)
            || (description && current.description.is_some())
            || (!description && current.result.is_some())
        {
            tx.rollback().await?;
            return Ok(());
        }
        let (namespace, subject) = current.job.subject_scope()?;
        let saved = tx.model_call(&call.id).await?;
        let expected_step = if description { "describe" } else { "structure" };
        if saved.job_id != run.job.id
            || saved.step_key != expected_step
            || saved.namespace_id != *namespace
            || saved.subject_id != *subject
            || saved.state != ModelCallState::Succeeded
        {
            return Err(Error::invalid(
                "media adoption requires its successful scoped paid step",
            ));
        }
        let metadata: InvocationMetadata<MediaModelInput> =
            serde_json::from_value(saved.request.clone())?;
        if serde_json::to_value(&metadata.input)? != serde_json::to_value(input)?
            || metadata.actual_access != input.access()
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "saved media invocation differs from its committed input directory",
            ));
        }
        let result = saved
            .response
            .as_ref()
            .and_then(|response| response.get("result"))
            .ok_or_else(|| Error::internal("successful media call has no parsed result"))?;
        let (target, payload, kind) = if description {
            let target = current
                .description_target
                .clone()
                .ok_or_else(|| Error::internal("description target missing"))?;
            let text: String = serde_json::from_value(result.clone())?;
            let kind = match (metadata.actual_access.visual, metadata.actual_access.audio) {
                (ChannelMode::Unavailable, ChannelMode::Original) => {
                    RepresentationKind::AudioDescription
                }
                (ChannelMode::Original, ChannelMode::Unavailable) => {
                    RepresentationKind::ImageDescription
                }
                _ => RepresentationKind::SceneDescription,
            };
            (target, RepresentationPayload::Text { text }, kind)
        } else {
            (
                current.result_target.clone(),
                RepresentationPayload::Interpretation {
                    value: serde_json::from_value(result.clone())?,
                },
                RepresentationKind::StructuredInterpretation,
            )
        };
        let expected_revision = target.revision - 1;
        let previous = if expected_revision > 0 {
            Some(RepresentationRef {
                id: target.id.clone(),
                revision: expected_revision,
            })
        } else {
            None
        };
        let completed = chrono::DateTime::parse_from_rfc3339(
            saved
                .completed_at
                .as_deref()
                .ok_or_else(|| Error::internal("paid step completion time is missing"))?,
        )
        .map_err(|error| Error::internal(error.to_string()))?
        .timestamp_millis();
        let preprocessing = current
            .prepared
            .as_ref()
            .ok_or_else(|| Error::internal("prepared inputs missing"))?
            .preprocessing_digest
            .clone();
        let prompt_digest = metadata
            .preview
            .prompt
            .as_ref()
            .map(|prompt| prompt.sha256.clone());
        let schema_digest = metadata.preview.output_schema_sha256.clone();
        let producer = DerivationProducer {
            signature: digest(
                &json!({"role":metadata.role,"resource":metadata.resource,"route":metadata.route,"prompt":prompt_digest,"schema":schema_digest,"config":metadata.config_digest,"preprocessing":preprocessing}),
            )?,
            implementation: metadata.role,
            model_call_id: Some(saved.id.clone()),
            prompt_digest,
            schema_digest,
            config_digest: metadata.config_digest,
            preprocessing_digest: Some(preprocessing),
        };
        let value = DerivedRepresentation {
            id: target.id,
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            revision: target.revision,
            inputs: input
                .catalog
                .iter()
                .enumerate()
                .map(|(ordinal, entry)| DerivationInput {
                    ordinal: ordinal as u32,
                    source: entry.source.clone(),
                    role: entry.role,
                })
                .collect(),
            strategy: current.request.strategy,
            kind,
            payload,
            producer,
            access: metadata.actual_access,
            observation_id: Some(current.request.observation_id.clone()),
            source_event_kind: Some(current.source_event_kind),
            timeline: Some(current.request.source.clone()),
            formed_at: TimePoint {
                domain: "unix_ms".into(),
                coordinate: completed,
            },
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
            previous,
            quality: input.quality.clone(),
        };
        value.validate_actual_access(metadata.actual_access, input.quality.source_duration_ms)?;
        tx.save_representation(&value, expected_revision).await?;
        let reference = value.reference();
        if description {
            tx.set_media_description(&current.job.id, &reference)
                .await?;
        }
        let final_result =
            !description || current.request.strategy == DerivationStrategy::DescriptionOnly;
        if final_result {
            tx.set_media_result(&current.job.id, &reference).await?;
            self.tasks.complete_in(&mut tx, &mut current.job, json!({"representation":reference,"description":if description { Some(reference.clone()) } else { current.description }})).await?;
        }
        tx.commit(json!({"representation":reference,"description_step":description,"complete":final_result})).await?;
        Ok(())
    }
}
