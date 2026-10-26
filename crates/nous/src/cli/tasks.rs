use super::op_id;
use crate::http::{CancelJob, Operation, tasks::WaitJobs};
use clap::Subcommand;

#[derive(Subcommand)]
pub(super) enum JobCommand {
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
