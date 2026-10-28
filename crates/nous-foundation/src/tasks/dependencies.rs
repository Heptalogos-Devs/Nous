use super::Tasks;
use nous_domain::*;
use nous_storage::WriteTransaction;
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyResolution {
    Ready,
    Suspended,
    Failed,
}
pub(super) enum DependencyStatus {
    Ready,
    Pending,
    Failed(Error),
}
pub(super) fn dependency_wait() -> WaitCondition {
    WaitCondition {
        kind: "dependencies".into(),
        requires_attention: false,
        resume: ResumeCondition::Ready,
        details: json!({}),
    }
}
impl Tasks {
    /// Stage the continuation and children in `tx`, then commit this decision.
    /// A suspended or failed handler returns immediately after commit to release its worker slot.
    pub async fn suspend_until_in(
        &self,
        tx: &mut WriteTransaction,
        parent: &mut JobRecord,
        required_ids: Vec<String>,
        policy: DependencyPolicy,
    ) -> Result<DependencyResolution> {
        tx.replace_job_dependencies(parent, required_ids, policy)
            .await?;
        let (resolution, state, wait, error) = match self.dependency_status_in(tx, parent).await? {
            DependencyStatus::Ready => (DependencyResolution::Ready, JobState::Running, None, None),
            DependencyStatus::Pending => (
                DependencyResolution::Suspended,
                JobState::Waiting,
                Some(dependency_wait()),
                None,
            ),
            DependencyStatus::Failed(error) => (
                DependencyResolution::Failed,
                JobState::Failed,
                None,
                Some(error),
            ),
        };
        self.change_in(tx, parent, state, wait, error).await?;
        Ok(resolution)
    }
    pub(super) async fn dependency_status_in(
        &self,
        tx: &mut WriteTransaction,
        parent: &JobRecord,
    ) -> Result<DependencyStatus> {
        let mut pending = false;
        let mut failed = Vec::new();
        for id in &parent.dependencies {
            let child = tx.job_record(id).await?;
            if child.namespace_id != parent.namespace_id || child.subject_id != parent.subject_id {
                return Err(Error::invalid("task dependency belongs to another scope"));
            }
            match child.state {
                JobState::Succeeded => {}
                JobState::Failed | JobState::Cancelled => {
                    failed.push(json!({"id":child.id,"state":child.state}))
                }
                _ => pending = true,
            }
        }
        if parent.dependency_policy == DependencyPolicy::AllSucceeded && !failed.is_empty() {
            return Ok(DependencyStatus::Failed(
                Error::new(ErrorCode::Conflict, "required tasks did not succeed")
                    .with_context("dependencies", json!(failed)),
            ));
        }
        Ok(if pending {
            DependencyStatus::Pending
        } else {
            DependencyStatus::Ready
        })
    }
}
