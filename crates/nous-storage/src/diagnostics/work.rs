use crate::{OperationReceipt, ReadView, storage_error};
use nous_domain::*;

impl ReadView {
    pub async fn subject_maintenance_receipts(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<Vec<OperationReceipt>> {
        let scope = format!("namespace:{namespace}/subject:{subject}");
        let receipts:Vec<String>=sqlx::query_scalar("SELECT receipt FROM operation_receipts WHERE operation='memory.maintain' AND scope=? ORDER BY commit_seq")
            .bind(scope).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        receipts
            .into_iter()
            .map(|value| serde_json::from_str(&value).map_err(Error::from))
            .collect()
    }
}
