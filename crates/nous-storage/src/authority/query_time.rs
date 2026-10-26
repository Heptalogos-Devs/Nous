use crate::{ReadView, storage_error};
use nous_domain::{OperationTime, Result};
use sqlx::Row;
impl ReadView {
    pub async fn commit_at_recorded(&mut self, at: &OperationTime) -> Result<i64> {
        let mut cursor = self.watermark;
        loop {
            let rows=sqlx::query("SELECT commit_seq,json_extract(receipt,'$.time') AS recorded FROM operation_receipts WHERE commit_seq<=? ORDER BY commit_seq DESC LIMIT 256")
                .bind(cursor).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
            if rows.is_empty() {
                return Ok(0);
            }
            for row in rows {
                let sequence: i64 = row.get("commit_seq");
                let recorded = OperationTime::parse(row.get("recorded"))?;
                if recorded.utc() <= at.utc() {
                    return Ok(sequence);
                }
                cursor = sequence - 1;
            }
        }
    }
}
