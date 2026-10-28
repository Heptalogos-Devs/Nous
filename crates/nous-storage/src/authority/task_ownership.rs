use crate::{WriteTransaction, storage_error};
use nous_domain::Result;

impl WriteTransaction {
    /// Ownership is immutable and established only after an existing parent, so
    /// this tree is acyclic. Dependencies never participate in this traversal.
    pub async fn owned_job_ids(&mut self, parent_id: &str) -> Result<Vec<String>> {
        sqlx::query_scalar("WITH RECURSIVE owned(id,depth) AS (SELECT job_id,0 FROM task_records WHERE job_id=? UNION ALL SELECT child.job_id,owned.depth+1 FROM task_records child JOIN owned ON child.parent_job_id=owned.id) SELECT id FROM owned ORDER BY depth DESC,id")
            .bind(parent_id).fetch_all(&mut *self.tx).await.map_err(storage_error)
    }
}
