use super::op_id;
use crate::http::{
    Operation,
    cognition::{CognitionOperation, EmbeddingSync},
};
use clap::Subcommand;
use nous_domain::SubjectId;

#[derive(Subcommand)]
pub enum EmbeddingCommand {
    /// Queue missing vectors for authorized cognition, media and tags.
    Sync {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long, default_value_t = 5000)]
        limit: usize,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Read a persistent embedding run and its exact joint inputs.
    Run { job: String },
}
impl EmbeddingCommand {
    pub fn operation(self) -> Operation {
        Operation::Cognition(match self {
            Self::Sync {
                subject,
                limit,
                operation_id,
            } => CognitionOperation::EmbeddingSync(EmbeddingSync {
                subject_id: subject.to_string(),
                limit,
                operation_id: op_id(operation_id),
            }),
            Self::Run { job } => CognitionOperation::EmbeddingRun(job),
        })
    }
}
