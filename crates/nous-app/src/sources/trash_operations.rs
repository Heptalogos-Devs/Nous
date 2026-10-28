use super::operations::authorize;
use crate::App;
use nous_domain::*;
use nous_storage::{ReadView, SourceOperationRecord, SourceTrashOperation};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceTrashOperationsPage {
    pub items: Vec<SourceTrashOperation>,
    pub next_before_commit_seq: Option<i64>,
    pub watermark: i64,
}
async fn authorize_operation(
    view: &mut ReadView,
    ctx: &CallContext,
    ns: &NamespaceId,
    sub: &SubjectId,
    record: &SourceTrashOperation,
) -> Result<()> {
    authorize(
        view,
        ctx,
        ns,
        sub,
        &SourceOperationRecord {
            operation: "source.trash".into(),
            receipt: record.trash.clone(),
        },
    )
    .await?;
    if let Some(restore) = &record.restore {
        authorize(
            view,
            ctx,
            ns,
            sub,
            &SourceOperationRecord {
                operation: "source.restore".into(),
                receipt: restore.clone(),
            },
        )
        .await?;
    }
    Ok(())
}
impl App {
    pub async fn source_trash_operation(
        &self,
        ctx: &CallContext,
        request: &SourceOperationRequest,
    ) -> Result<SourceTrashOperation> {
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
            .source_trash_operation_records(
                &request.namespace_id,
                &request.subject_id,
                Some(before),
                1,
            )
            .await?
            .into_iter()
            .find(|record| record.trash.commit_seq == request.commit_seq)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "source trash operation not found"))?;
        authorize_operation(
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
    pub async fn source_trash_operations(
        &self,
        ctx: &CallContext,
        request: &SourceOperationsRequest,
    ) -> Result<SourceTrashOperationsPage> {
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
                .source_trash_operation_records(
                    &request.namespace_id,
                    &request.subject_id,
                    before,
                    200,
                )
                .await?;
            if records.is_empty() {
                break;
            }
            for record in records {
                before = Some(record.trash.commit_seq);
                match authorize_operation(
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
            items.last().map(|record| record.trash.commit_seq)
        } else {
            None
        };
        let result = SourceTrashOperationsPage {
            items,
            next_before_commit_seq,
            watermark: view.watermark,
        };
        view.finish().await?;
        Ok(result)
    }
}
