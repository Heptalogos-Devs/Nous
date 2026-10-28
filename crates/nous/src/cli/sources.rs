use super::{op_id, parse_time};
use crate::http::sources::SourceManagementOperation;
use crate::http::{Operation, RecordObservation, sources::RetrievalSourceRequest};
use anyhow::Result;
use clap::Subcommand;
use nous_domain::{
    ConversationRef, ExternalObjectRef, MaterialId, NamespaceId, ObservationId, ObservationOrigin,
    RestoreSourcesRequest, SourceDirectoryRequest, SourceOperationRequest, SourceOperationsRequest,
    SubjectId, TrashSourcesRequest, references::ReferenceLocator,
};

#[derive(Subcommand)]
pub enum ObservationCommand {
    Record {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        material: MaterialId,
        /// JSON time point; omitted means the contact time was not supplied.
        #[arg(long)]
        observed_at: Option<String>,
        #[arg(long)]
        conversation_ref: Option<ConversationRef>,
        #[arg(long)]
        external_object_ref: Option<ExternalObjectRef>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    List {
        #[arg(long)]
        subject: SubjectId,
    },
}
impl ObservationCommand {
    pub fn operation(self) -> Result<Operation> {
        Ok(match self {
            Self::Record {
                subject,
                material,
                observed_at,
                conversation_ref,
                external_object_ref,
                operation_id,
            } => Operation::RecordObservation(RecordObservation {
                operation_id: op_id(operation_id),
                subject_id: subject.to_string(),
                material_id: material.to_string(),
                observed_at: parse_time(observed_at)?,
                origin: ObservationOrigin::from_refs(conversation_ref, external_object_ref)?,
            }),
            Self::List { subject } => Operation::ListObservations(subject.to_string()),
        })
    }
}
#[derive(Subcommand)]
pub enum SourceCommand {
    /// Preview the exact source trash plan without committing changes.
    Preview {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long = "source", required = true)]
        sources: Vec<String>,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Inspect a logical trash operation and its actual restore receipt.
    TrashOperation {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        commit_seq: i64,
    },
    /// List logical trash batches and their current restored state.
    TrashOperations {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        before_commit_seq: Option<i64>,
    },
    /// Read an exact source trash or restore event.
    Operation {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        commit_seq: i64,
    },
    /// List authorized source operation receipts in snapshot order.
    Operations {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        before_commit_seq: Option<i64>,
    },
    /// List exact named source groups visible to this caller.
    List {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
    },
    /// Move all selected source records to trash in one operation.
    Trash {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long = "source", required = true)]
        sources: Vec<String>,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "")]
        reason: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Restore the exact batch identified by the trash receipt's commit_seq.
    Restore {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        trash_commit_seq: i64,
        #[arg(long, default_value = "")]
        reason: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Read {
        reference: ReferenceLocator,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        observation: Option<ObservationId>,
    },
}
impl SourceCommand {
    pub fn operation(self) -> Operation {
        match self {
            Self::Preview {
                namespace,
                subject,
                sources,
                force,
                reason,
            } => Operation::SourceManagement(Box::new(SourceManagementOperation::Preview {
                request: TrashSourcesRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    sources,
                    force,
                    reason,
                },
            })),
            Self::TrashOperation {
                namespace,
                subject,
                commit_seq,
            } => Operation::SourceManagement(Box::new(
                SourceManagementOperation::GetTrashOperation {
                    request: SourceOperationRequest {
                        namespace_id: namespace,
                        subject_id: subject,
                        commit_seq,
                    },
                },
            )),
            Self::TrashOperations {
                namespace,
                subject,
                limit,
                before_commit_seq,
            } => Operation::SourceManagement(Box::new(
                SourceManagementOperation::ListTrashOperations {
                    request: SourceOperationsRequest {
                        namespace_id: namespace,
                        subject_id: subject,
                        limit,
                        before_commit_seq,
                    },
                },
            )),
            Self::Operation {
                namespace,
                subject,
                commit_seq,
            } => Operation::SourceManagement(Box::new(SourceManagementOperation::Get {
                request: SourceOperationRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    commit_seq,
                },
            })),
            Self::Operations {
                namespace,
                subject,
                limit,
                before_commit_seq,
            } => Operation::SourceManagement(Box::new(SourceManagementOperation::ListOperations {
                request: SourceOperationsRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    limit,
                    before_commit_seq,
                },
            })),
            Self::List { namespace, subject } => {
                Operation::SourceManagement(Box::new(SourceManagementOperation::List {
                    request: SourceDirectoryRequest {
                        namespace_id: namespace,
                        subject_id: subject,
                    },
                }))
            }
            Self::Trash {
                namespace,
                subject,
                sources,
                force,
                reason,
                operation_id,
            } => Operation::SourceManagement(Box::new(SourceManagementOperation::Trash {
                operation_id: op_id(operation_id),
                request: TrashSourcesRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    sources,
                    force,
                    reason,
                },
            })),
            Self::Restore {
                namespace,
                subject,
                trash_commit_seq,
                reason,
                operation_id,
            } => Operation::SourceManagement(Box::new(SourceManagementOperation::Restore {
                operation_id: op_id(operation_id),
                request: RestoreSourcesRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    trash_commit_seq,
                    reason,
                },
            })),
            Self::Read {
                reference,
                subject,
                observation,
            } => Operation::ReadSource(RetrievalSourceRequest {
                subject_id: subject,
                reference,
                observation_id: observation,
            }),
        }
    }
}
