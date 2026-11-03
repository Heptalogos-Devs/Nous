//! Request-local plans expose canonical keys, never generation-local numeric IDs.
use super::*;

impl ServingGeneration {
    pub(super) fn thinking_output(
        &self,
        value: nous_vcp::thinking::ThinkingOutput,
    ) -> Result<Value> {
        let candidate = |id: i64| -> Result<&str> {
            Ok(self
                .mappings
                .candidates
                .get(&id)
                .context("THINKING_CANDIDATE_UNKNOWN: selection escaped snapshot")?
                .id
                .as_str())
        };
        let node = |id: i64| -> Result<&str> {
            Ok(self
                .mappings
                .tags
                .get(&id)
                .context("THINKING_NODE_UNKNOWN: route escaped snapshot")?
                .as_str())
        };
        let stages = value.stages.iter().map(|stage| {
            let selections = stage.results.iter().map(|item| {
                let route = item.route.as_ref().map(|edge| -> Result<Value> {
                    Ok(json!({"source":node(edge.source_id)?,"target":node(edge.target_id)?,
                        "previous":edge.previous_id.map(&node).transpose()?,"hop":edge.hop,
                        "flow":edge.flow,"wormhole":edge.wormhole,"immediate_return":edge.immediate_return}))
                }).transpose()?;
                Ok(json!({"reference":candidate(item.chunk_id)?,"score":item.score,
                    "parents":item.parents.iter().map(|id|candidate(*id)).collect::<Result<Vec<_>>>()?,
                    "relation":item.relation,"route":route}))
            }).collect::<Result<Vec<_>>>()?;
            Ok(json!({"name":stage.diary_name,"limit":stage.k,"selections":selections}))
        }).collect::<Result<Vec<_>>>()?;
        Ok(json!({"status":"ready","generation":value.generation,
            "artifact_signature":value.artifact_sig,"stages":stages}))
    }
}
