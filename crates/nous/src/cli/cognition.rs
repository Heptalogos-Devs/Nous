use anyhow::Result;
use clap::{Args, Subcommand};
use nous_domain::{
    CognitiveKind, CognitiveRef, MemoryId, ModelCallId, ObservationId, QueryMode, QueryRequest,
    SubjectId, TimeFilter,
};

use crate::{
    cli::op_id,
    http::Operation,
    http::cognition::{CognitionOperation, DeleteMemory, EmbeddingSync, FormMemory},
};

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// Queue missing text embeddings for a Subject's current cognition and tags.
    Embed {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long, default_value_t = 5000)]
        limit: usize,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Apply a revision-checked lifecycle or content change.
    Change(crate::cli::maintenance::ChangeArgs),
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
    /// Queue formation from a recorded contact or import and an optional exact media source.
    Form {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        observation: ObservationId,
        /// JSON MediaReference; omitted selects the observation's original material.
        #[arg(long)]
        source: Option<String>,
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
                return Ok(crate::cli::maintenance::recycle(subject, limit));
            }
            Self::Audit { id, limit } => return Ok(crate::cli::maintenance::audit(id, limit)),
            Self::Maintain {
                subject,
                limit,
                operation_id,
                after,
                snapshot_seq,
            } => {
                return Ok(crate::cli::maintenance::maintain(
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
                source,
                operation_id,
            } => CognitionOperation::Form(FormMemory {
                operation_id: op_id(operation_id),
                subject_id: subject.to_string(),
                observation_id: observation.to_string(),
                source: source
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
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
    #[arg(long, value_parser=parse_query_mode, default_value = "lexical")]
    mode: QueryMode,
    #[arg(long)]
    include_inactive: bool,
    /// Natural-language intent. Exact references use kind:UUID@revision.
    #[arg(default_value = "")]
    text: String,
    #[arg(long)]
    subject: SubjectId,
    #[arg(long)]
    reference: Option<CognitiveRef>,
    #[arg(long)]
    source: Option<String>,
    #[arg(long="kind", value_parser=parse_kind)]
    kinds: Vec<CognitiveKind>,
    #[arg(long)]
    as_of_commit: Option<i64>,
    #[arg(long)]
    include_deleted: bool,
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
        let time: Option<TimeFilter> = self
            .time
            .map(|time| serde_json::from_str(&time))
            .transpose()?;
        Ok(Operation::Cognition(CognitionOperation::Query(
            QueryRequest {
                mode: self.mode,
                include_inactive: self.include_inactive,
                subject_id: self.subject,
                text: self.text,
                reference: self.reference,
                kinds: self.kinds,
                as_of_commit: self.as_of_commit,
                include_deleted: self.include_deleted,
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
    /// Show the role's typed input contract, requirements and Prompt binding.
    Contract {
        #[arg(long)]
        role: String,
        /// Typed role input used to build the dynamic messages and output contract.
        #[arg(long)]
        file: Option<std::path::PathBuf>,
        #[command(flatten)]
        scope: super::scope::ScopeArgs,
    },
    /// Build the actual invocation locally in the service without a provider call.
    Preview {
        #[arg(long)]
        role: String,
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long)]
        prompt_file: Option<std::path::PathBuf>,
        #[command(flatten)]
        scope: super::scope::ScopeArgs,
    },
    /// Read an authorized model call and its recorded provider evidence.
    Call { id: ModelCallId },
    /// Inspect the service's configured formation resource and readiness.
    Status,
    /// Check local model resource configuration and credential availability without a model request.
    Check,
}

fn parse_query_mode(value: &str) -> Result<QueryMode, String> {
    serde_json::from_value(serde_json::json!(value)).map_err(|error| error.to_string())
}
fn parse_kind(value: &str) -> Result<CognitiveKind, String> {
    serde_json::from_value(serde_json::json!(value)).map_err(|error| error.to_string())
}
