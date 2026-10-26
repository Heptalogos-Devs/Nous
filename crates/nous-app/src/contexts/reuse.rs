use super::*;
use nous_foundation::models::{InvocationEdits, InvocationMetadata};

impl App {
    pub(super) async fn reusable_context_steward(
        &self,
        preparation: &ContextPreparation,
        input: &model::StewardInput,
    ) -> Result<Option<(model::StewardProposal, ModelCallId)>> {
        let Some(cursor) = &preparation.request.known_cursor else {
            return Ok(None);
        };
        let previous = match self
            .database
            .context_projection(&cursor.projection_id)
            .await
        {
            Ok(previous) => previous,
            Err(error) if error.code == ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let (ns, subject) = preparation.job.subject_scope()?;
        if previous.namespace_id != *ns
            || previous.subject_id != *subject
            || previous.session_id != preparation.request.session_id
            || previous.consumer_id != preparation.request.consumer_id
            || previous.instance_id != preparation.request.instance_id
            || Some(&previous.config_digest) != preparation.config_digest.as_ref()
        {
            return Ok(None);
        }
        let Some(id) = previous.steward_call else {
            return Ok(None);
        };
        let call = match self
            .calls
            .read(&preparation.job.call_context("context")?, &id)
            .await
        {
            Ok(call) => call,
            Err(error) if error.code == ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if call.state != ModelCallState::Succeeded {
            return Ok(None);
        }
        let Ok(metadata) =
            serde_json::from_value::<InvocationMetadata<model::StewardInput>>(call.request.clone())
        else {
            return Ok(None);
        };
        if metadata.input.intent != input.intent
            || serde_json::to_value(&metadata.input.context)?
                != serde_json::to_value(&input.context)?
            || serde_json::to_value(&metadata.input.sources)?
                != serde_json::to_value(&input.sources)?
        {
            return Ok(None);
        }
        let preview = match self
            .model
            .preview(
                &ConfigScope::Subject {
                    namespace_id: ns.clone(),
                    subject_id: subject.clone(),
                },
                model::ROLE,
                serde_json::to_value(input)?,
                InvocationEdits::default(),
            )
            .await
        {
            Ok(preview) => preview,
            Err(_) => return Ok(None),
        };
        if serde_json::to_value(&preview.input)? != serde_json::to_value(&metadata.preview.input)? {
            return Ok(None);
        }
        let Some(selected) = metadata
            .preview
            .routes
            .iter()
            .find(|route| route.route == metadata.route)
        else {
            return Ok(None);
        };
        let compatible = preview
            .routes
            .iter()
            .any(|route| route.invocation_digest == selected.invocation_digest);
        if !compatible {
            return Ok(None);
        }
        let Some(result) = call
            .response
            .as_ref()
            .and_then(|response| response.get("result"))
        else {
            return Ok(None);
        };
        let Ok(proposal) = serde_json::from_value::<model::StewardProposal>(result.clone()) else {
            return Ok(None);
        };
        if proposal.validate(input).is_err() {
            return Ok(None);
        }
        Ok(Some((proposal, id)))
    }
}
