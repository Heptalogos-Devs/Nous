use crate::{WriteTransaction, codec::enum_name, storage_error, write_error};
use nous_domain::*;
use std::collections::BTreeSet;

impl WriteTransaction {
    /// Only this operation may replace the dependency edges of an accepted task.
    /// The caller stages checkpoint/result and lifecycle changes in this same transaction.
    pub async fn replace_job_dependencies(
        &mut self,
        parent: &mut JobRecord,
        required: Vec<String>,
        policy: DependencyPolicy,
    ) -> Result<()> {
        let stored = self.job_record(&parent.id).await?;
        stored.require_read(self.context())?;
        if stored.state != JobState::Running
            || parent.state != JobState::Running
            || stored.dependencies != parent.dependencies
            || stored.dependency_policy != parent.dependency_policy
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "dependency replacement requires the current running task",
            ));
        }
        let required: Vec<_> = required
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for id in &required {
            super::tasks::validate_job_id(id)?;
            if id == &stored.id {
                return Err(Error::invalid("a task cannot depend on itself"));
            }
            let child = self.job_record(id).await?;
            if child.namespace_id != stored.namespace_id || child.subject_id != stored.subject_id {
                return Err(Error::invalid("task dependency belongs to another scope"));
            }
        }
        let encoded = serde_json::to_string(&required)?;
        let cycle:bool=sqlx::query_scalar("WITH RECURSIVE reachable(id) AS (SELECT value FROM json_each(?) UNION SELECT edge.value FROM reachable r JOIN task_records t ON t.job_id=r.id JOIN json_each(t.dependencies) edge) SELECT EXISTS(SELECT 1 FROM reachable WHERE id=?)")
            .bind(&encoded).bind(&stored.id).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
        if cycle {
            return Err(Error::invalid(
                "task dependency replacement creates a cycle",
            ));
        }
        sqlx::query("UPDATE task_records SET dependencies=?,dependency_policy=? WHERE job_id=?")
            .bind(encoded)
            .bind(enum_name(&policy)?)
            .bind(&stored.id)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        parent.dependencies = required;
        parent.dependency_policy = policy;
        Ok(())
    }
}
