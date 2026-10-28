mod inventory;
mod vectors;
mod work;
use crate::{ReadView, storage_error};
use nous_domain::*;

impl ReadView {
    pub async fn subject_model_call_ids(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<Vec<ModelCallId>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT id FROM model_calls WHERE namespace_id=? AND subject_id=? ORDER BY id",
        )
        .bind(namespace.as_str())
        .bind(subject.as_str())
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(ModelCallId)
        .collect())
    }
}
