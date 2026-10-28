use crate::App;
use nous_domain::*;
use nous_storage::{BeginOperation, operation_key};

fn original(mut references: Vec<CognitiveRef>) -> Result<Vec<CognitiveRef>> {
    for reference in &mut references {
        reference.revision = reference
            .revision
            .checked_sub(1)
            .filter(|revision| *revision > 0)
            .ok_or_else(|| Error::internal("source preview produced no original revision"))?;
    }
    Ok(references)
}
impl App {
    pub async fn preview_sources(
        &self,
        ctx: &CallContext,
        mut request: TrashSourcesRequest,
    ) -> Result<SourceTrashPreview> {
        request.validate()?;
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        request.sources.sort();
        request.sources.dedup();
        let scope = format!(
            "namespace:{}/subject:{}",
            request.namespace_id, request.subject_id
        );
        let key = operation_key(
            ctx,
            "source.preview",
            &scope,
            &ulid::Ulid::generate().to_string(),
            &request,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::New(tx) => tx,
            BeginOperation::Replay(_) => {
                return Err(Error::internal(
                    "ephemeral source preview identity was reused",
                ));
            }
        };
        let watermark = tx.commit_seq() - 1;
        let result = self.trash_sources_in(ctx, &mut tx, request).await?;
        tx.rollback().await?;
        Ok(SourceTrashPreview {
            watermark,
            sources: result.sources,
            matched_sources: result.matched_sources,
            memory_targets: original(result.trashed_memories)?,
            graph: SourceGraphPlan {
                revise_entities: original(result.graph.revised_entities)?,
                retire_entities: original(result.graph.retired_entities)?,
                revise_associations: original(result.graph.revised_associations)?,
                retire_associations: original(result.graph.retired_associations)?,
                retire_annotations: original(result.graph.retired_annotations)?,
            },
            affected_independent: result.affected_independent,
            force: result.force,
            vector_associations_to_retire: result.retired_vector_associations,
        })
    }
}
