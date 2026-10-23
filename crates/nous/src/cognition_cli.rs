use anyhow::Result;
use clap::{Args, Subcommand};
use nous_domain::{MemoryId, ModelCallId, ObservationId, SubjectId};

use crate::{
    api::Operation,
    cognition_api::{
        CognitionOperation, DeleteMemory, EmbeddingSync, FormMemory, QueryInput, QueryModeInput,
        QueryTimeFilter,
    },
    op_id,
};

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// Queue missing text embeddings for a Subject's current memories and tags.
    Embed {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long, default_value_t = 5000)]
        limit: usize,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Apply a revision-checked lifecycle or content change.
    Change(crate::maintenance_cli::ChangeArgs),
    RecycleBin {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Audit {
        id: MemoryId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Maintain {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        operation_id: Option<String>,
        #[arg(long)]
        after: Option<MemoryId>,
        #[arg(long)]
        snapshot_seq: Option<i64>,
    },
    /// Queue model formation from a real recorded contact.
    Form {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        observation: ObservationId,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Get {
        id: MemoryId,
    },
    History {
        id: MemoryId,
    },
    /// Create a retained deletion revision. The expected current revision is required.
    Delete {
        id: MemoryId,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Read original material, contact, formation record and model call.
    Source {
        id: MemoryId,
    },
}

impl MemoryCommand {
    pub fn operation(self) -> Result<Operation> {
        let operation = match self {
            Self::Embed {
                subject,
                limit,
                operation_id,
            } => CognitionOperation::EmbeddingSync(EmbeddingSync {
                subject_id: subject.to_string(),
                limit,
                operation_id: op_id(operation_id),
            }),
            Self::Change(input) => return input.operation(),
            Self::RecycleBin { subject, limit } => {
                return Ok(crate::maintenance_cli::recycle(subject, limit));
            }
            Self::Audit { id, limit } => return Ok(crate::maintenance_cli::audit(id, limit)),
            Self::Maintain {
                subject,
                limit,
                operation_id,
                after,
                snapshot_seq,
            } => {
                return Ok(crate::maintenance_cli::maintain(
                    subject,
                    limit,
                    operation_id,
                    after,
                    snapshot_seq,
                ));
            }
            Self::Form {
                subject,
                observation,
                operation_id,
            } => CognitionOperation::Form(FormMemory {
                operation_id: op_id(operation_id),
                subject_id: subject.to_string(),
                observation_id: observation.to_string(),
            }),
            Self::Get { id } => CognitionOperation::Get(id.to_string()),
            Self::History { id } => CognitionOperation::History(id.to_string()),
            Self::Delete {
                id,
                expected_revision,
                operation_id,
            } => CognitionOperation::Delete(
                id.to_string(),
                DeleteMemory {
                    operation_id: op_id(operation_id),
                    expected_revision,
                },
            ),
            Self::Source { id } => CognitionOperation::Source(id.to_string()),
        };
        Ok(Operation::Cognition(operation))
    }
}

#[derive(Args)]
pub struct QueryArgs {
    #[arg(long, value_enum, default_value = "lexical")]
    mode: QueryModeInput,
    #[arg(long)]
    include_inactive: bool,
    /// Natural-language intent. Supply --reference for an exact authority read.
    #[arg(default_value = "")]
    text: String,
    #[arg(long)]
    subject: SubjectId,
    #[arg(long)]
    reference: Option<MemoryId>,
    #[arg(long)]
    source: Option<String>,
    /// JSON {"axis":"event","range":{"domain":"utc","start":0,"end":123}}.
    #[arg(long)]
    time: Option<String>,
    #[arg(long, default_value_t = 10)]
    limit: usize,
    #[arg(long)]
    min_commit_seq: Option<i64>,
}
impl QueryArgs {
    pub fn operation(self) -> Result<Operation> {
        let time: Option<QueryTimeFilter> = self
            .time
            .map(|time| serde_json::from_str(&time))
            .transpose()?;
        Ok(Operation::Cognition(CognitionOperation::Query(
            QueryInput {
                mode: self.mode,
                include_inactive: self.include_inactive,
                subject_id: self.subject.to_string(),
                text: self.text,
                reference: self.reference.map(|id| id.to_string()),
                source: self.source,
                time,
                limit: self.limit,
                min_commit_seq: self.min_commit_seq,
            },
        )))
    }
}

#[derive(Subcommand)]
pub enum ModelCommand {
    /// Read an authorized model call and its recorded provider evidence.
    Call { id: ModelCallId },
    /// Inspect the service's configured formation resource and readiness.
    Status,
    /// Check local model resource configuration and credential availability without a model request.
    Check,
}
