use super::op_id;
use crate::http::{CancelJob, Operation, tasks::WaitJobs};
use clap::Subcommand;
use nous_domain::{JobCursor, JobListRequest, JobState, NamespaceId, SubjectId};

#[derive(Subcommand)]
pub(super) enum JobCommand {
    /// List readable task summaries in stable acceptance order.
    List {
        #[arg(long)]
        namespace: Option<NamespaceId>,
        #[arg(long)]
        subject: Option<SubjectId>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long,value_parser=parse_state)]
        state: Option<JobState>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// JSON next_cursor returned by the preceding page.
        #[arg(long,value_parser=parse_cursor)]
        cursor: Option<JobCursor>,
    },
    Get {
        id: String,
    },
    Cancel {
        id: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Wait {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
        #[arg(long)]
        timeout_ms: u64,
    },
}
impl JobCommand {
    pub(super) fn operation(self) -> Operation {
        match self {
            Self::List {
                namespace,
                subject,
                kind,
                state,
                limit,
                cursor,
            } => Operation::ListJobs(JobListRequest {
                namespace_id: namespace,
                subject_id: subject,
                kind,
                state,
                limit,
                cursor,
            }),
            Self::Get { id } => Operation::GetJob(id),
            Self::Cancel { id, operation_id } => Operation::CancelJob(
                id,
                CancelJob {
                    operation_id: op_id(operation_id),
                },
            ),
            Self::Wait { ids, timeout_ms } => Operation::WaitJobs(WaitJobs { ids, timeout_ms }),
        }
    }
}

fn parse_state(value: &str) -> Result<JobState, String> {
    serde_json::from_value(serde_json::Value::String(value.into())).map_err(|e| e.to_string())
}
fn parse_cursor(value: &str) -> Result<JobCursor, String> {
    serde_json::from_str(value).map_err(|e| e.to_string())
}
