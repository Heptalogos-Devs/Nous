//! Logical source deletion operations projected from immutable transaction receipts.
use crate::{OperationReceipt, ReadView, storage_error};
use nous_domain::*;
use sqlx::Row;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceTrashOperation {
    pub trash: OperationReceipt,
    pub restore: Option<OperationReceipt>,
}
impl ReadView {
    pub async fn source_trash_operation_records(
        &mut self,
        ns: &NamespaceId,
        sub: &SubjectId,
        before: Option<i64>,
        limit: usize,
    ) -> Result<Vec<SourceTrashOperation>> {
        let scope = format!("namespace:{ns}/subject:{sub}");
        let rows = sqlx::query("SELECT t.receipt AS trash,(SELECT r.receipt FROM operation_receipts r WHERE r.scope=t.scope AND r.operation='source.restore' AND json_extract(r.receipt,'$.result.trash_commit_seq')=t.commit_seq AND r.commit_seq<=? ORDER BY r.commit_seq LIMIT 1) AS restore FROM operation_receipts t WHERE t.scope=? AND t.operation='source.trash' AND t.commit_seq<=? AND (? IS NULL OR t.commit_seq<?) ORDER BY t.commit_seq DESC LIMIT ?")
            .bind(self.watermark).bind(scope).bind(self.watermark).bind(before).bind(before).bind(limit as i64)
            .fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(SourceTrashOperation {
                    trash: serde_json::from_str(&row.get::<String, _>("trash"))?,
                    restore: row
                        .get::<Option<String>, _>("restore")
                        .map(|body| serde_json::from_str(&body))
                        .transpose()?,
                })
            })
            .collect()
    }
}
