use crate::App;
use nous_domain::*;
use nous_storage::{ReadView, SourceOperationRecord};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceOperationsPage {
    pub items: Vec<SourceOperationRecord>,
    pub next_before_commit_seq: Option<i64>,
    pub watermark: i64,
}
pub(super) async fn authorize(
    view: &mut ReadView,
    ctx: &CallContext,
    ns: &NamespaceId,
    sub: &SubjectId,
    record: &SourceOperationRecord,
) -> Result<()> {
    let references = match record.operation.as_str() {
        "source.trash" => {
            serde_json::from_value::<TrashSourcesResult>(record.receipt.result.clone())?
                .trashed_memories
        }
        "source.restore" => {
            serde_json::from_value::<RestoreSourcesResult>(record.receipt.result.clone())?
                .restored_memories
        }
        _ => return Err(Error::internal("unexpected source operation")),
    };
    if references.is_empty() && !ctx.source_read.is_empty() {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "empty source operation requires unrestricted source access",
        ));
    }
    let sources = references
        .into_iter()
        .map(|reference| SourceReadRef::Cognition { reference })
        .collect::<Vec<_>>();
    view.require_source_provenance(ctx, ns, sub, &sources).await
}
impl App {
    pub async fn source_operation(
        &self,
        ctx: &CallContext,
        request: &SourceOperationRequest,
    ) -> Result<SourceOperationRecord> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        let before = request
            .commit_seq
            .checked_add(1)
            .filter(|_| request.commit_seq > 0)
            .ok_or_else(|| {
                Error::invalid("source operation requires a positive commit sequence")
            })?;
        let mut view = self.database.read_view().await?;
        let record = view
            .source_operation_records(&request.namespace_id, &request.subject_id, Some(before), 1)
            .await?
            .into_iter()
            .find(|record| record.receipt.commit_seq == request.commit_seq)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "source operation not found"))?;
        authorize(
            &mut view,
            ctx,
            &request.namespace_id,
            &request.subject_id,
            &record,
        )
        .await?;
        view.finish().await?;
        Ok(record)
    }
    pub async fn source_operations(
        &self,
        ctx: &CallContext,
        request: &SourceOperationsRequest,
    ) -> Result<SourceOperationsPage> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        if !(1..=200).contains(&request.limit)
            || request.before_commit_seq.is_some_and(|seq| seq <= 0)
        {
            return Err(Error::invalid(
                "source operations require limit 1..200 and a positive before commit",
            ));
        }
        let mut view = self.database.read_view().await?;
        view.subject(&request.namespace_id, &request.subject_id)
            .await?;
        let mut before = request.before_commit_seq;
        let mut items = Vec::new();
        'scan: loop {
            let records = view
                .source_operation_records(&request.namespace_id, &request.subject_id, before, 200)
                .await?;
            if records.is_empty() {
                break;
            }
            for record in records {
                before = Some(record.receipt.commit_seq);
                match authorize(
                    &mut view,
                    ctx,
                    &request.namespace_id,
                    &request.subject_id,
                    &record,
                )
                .await
                {
                    Ok(()) => items.push(record),
                    Err(error) if error.code == ErrorCode::Forbidden => continue,
                    Err(error) => return Err(error),
                }
                if items.len() > request.limit {
                    break 'scan;
                }
            }
        }
        let next_before_commit_seq = if items.len() > request.limit {
            items.truncate(request.limit);
            items.last().map(|record| record.receipt.commit_seq)
        } else {
            None
        };
        let result = SourceOperationsPage {
            items,
            next_before_commit_seq,
            watermark: view.watermark,
        };
        view.finish().await?;
        Ok(result)
    }
}
