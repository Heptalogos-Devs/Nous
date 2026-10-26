use super::{op_id, print_result};
use crate::http::{ApiClient, Operation, contexts::ContextOperation};
use anyhow::{Context, Result};
use clap::Subcommand;
use nous_domain::{context_work::*, *};
use serde::de::DeserializeOwned;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ContextCommand {
    Schema,
    /// Submit any typed context operation from a JSON file.
    Apply {
        file: PathBuf,
    },
    /// Prepare cognition using a ContextPrepareRequest JSON file.
    Prepare {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Get {
        id: ContextPreparationId,
    },
    Cancel {
        id: ContextPreparationId,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Admit {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Admission {
        id: ContextAdmissionId,
    },
    Report {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Work {
        #[command(subcommand)]
        command: WorkCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Residents {
        session: ContextSessionId,
        #[arg(long)]
        include_evicted: bool,
    },
    ChangeResidents {
        session: ContextSessionId,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
#[derive(Subcommand)]
pub enum WorkCommand {
    Create {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        purpose: String,
        #[arg(long, default_value = "")]
        text: String,
        #[arg(long = "question")]
        questions: Vec<String>,
        #[arg(long = "anchor")]
        anchors: Vec<RetrievalTarget>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Get {
        id: WorkContextId,
    },
    List {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    History {
        id: WorkContextId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Apply a WorkContextChange: edit, pin, unpin, clear, pause, resume or end.
    Change {
        id: WorkContextId,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
#[derive(Subcommand)]
pub enum SessionCommand {
    Open {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        native_session: String,
        #[arg(long)]
        history_generation: String,
        #[arg(long)]
        parent: Option<ContextSessionId>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Get {
        id: ContextSessionId,
    },
    SetWork {
        id: ContextSessionId,
        #[arg(long)]
        work: Option<WorkContextId>,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Close {
        id: ContextSessionId,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
impl ContextCommand {
    pub fn operation(self) -> Result<Option<ContextOperation>> {
        Ok(Some(match self {
            Self::Schema => return Ok(None),
            Self::Apply { file } => read(file)?,
            Self::Prepare { file, operation_id } => ContextOperation::Prepare {
                operation_id: op_id(operation_id),
                request: Box::new(read(file)?),
            },
            Self::Get { id } => ContextOperation::Preparation { id },
            Self::Cancel { id, operation_id } => ContextOperation::CancelPreparation {
                id,
                operation_id: op_id(operation_id),
            },
            Self::Admit { file, operation_id } => ContextOperation::Admit {
                operation_id: op_id(operation_id),
                request: read(file)?,
            },
            Self::Admission { id } => ContextOperation::Admission { id },
            Self::Report { file, operation_id } => ContextOperation::Report {
                operation_id: op_id(operation_id),
                request: read(file)?,
            },
            Self::Work { command } => command.operation()?,
            Self::Session { command } => command.operation(),
            Self::Residents {
                session,
                include_evicted,
            } => ContextOperation::Residents {
                session_id: session,
                include_evicted,
            },
            Self::ChangeResidents {
                session,
                expected_revision,
                file,
                operation_id,
            } => ContextOperation::ChangeResidents {
                operation_id: op_id(operation_id),
                session_id: session,
                expected_revision,
                change: read(file)?,
            },
        }))
    }
}
impl WorkCommand {
    fn operation(self) -> Result<ContextOperation> {
        Ok(match self {
            Self::Create {
                subject,
                purpose,
                text,
                questions,
                anchors,
                operation_id,
            } => ContextOperation::CreateWork {
                operation_id: op_id(operation_id),
                request: CreateWorkContextRequest {
                    subject_id: subject,
                    content: WorkContextContent {
                        purpose,
                        context_text: text,
                        unresolved_questions: questions,
                        constraints: serde_json::Value::Null,
                        resume_conditions: Vec::new(),
                        budget_summary: serde_json::Value::Null,
                        anchors,
                    },
                },
            },
            Self::Get { id } => ContextOperation::ReadWork { id },
            Self::List { subject, limit } => ContextOperation::ListWork {
                subject_id: subject,
                limit,
            },
            Self::History { id, limit } => ContextOperation::WorkHistory { id, limit },
            Self::Change {
                id,
                expected_revision,
                file,
                operation_id,
            } => ContextOperation::ChangeWork {
                operation_id: op_id(operation_id),
                id,
                expected_revision,
                change: read(file)?,
            },
        })
    }
}
impl SessionCommand {
    fn operation(self) -> ContextOperation {
        match self {
            Self::Open {
                subject,
                native_session,
                history_generation,
                parent,
                operation_id,
            } => ContextOperation::OpenSession {
                operation_id: op_id(operation_id),
                request: OpenContextSessionRequest {
                    subject_id: subject,
                    native_session,
                    parent_session: parent,
                    history_generation,
                },
            },
            Self::Get { id } => ContextOperation::ReadSession { id },
            Self::SetWork {
                id,
                work,
                expected_revision,
                operation_id,
            } => ContextOperation::ChangeSession {
                operation_id: op_id(operation_id),
                id,
                expected_revision,
                change: ContextSessionChange {
                    active_work_context: work,
                    close: false,
                },
            },
            Self::Close {
                id,
                expected_revision,
                operation_id,
            } => ContextOperation::ChangeSession {
                operation_id: op_id(operation_id),
                id,
                expected_revision,
                change: ContextSessionChange {
                    active_work_context: None,
                    close: true,
                },
            },
        }
    }
}
fn read<T: DeserializeOwned>(file: PathBuf) -> Result<T> {
    serde_json::from_reader(std::fs::File::open(&file)?)
        .with_context(|| format!("invalid context operation {}", file.display()))
}
pub async fn execute(client: ApiClient, operation: ContextOperation) -> Result<()> {
    print_result(
        client
            .execute(&Operation::Context(Box::new(operation)))
            .await?,
    )
}
