use super::op_id;
use crate::http::Operation;
use clap::Subcommand;
use nous_app::references::ReferenceOperation;
use nous_domain::{NamespaceId, SubjectId, references::ReferenceLocator};
#[derive(Subcommand)]
pub enum ReferenceCommand {
    /// Resolve a returned readable address using the same Subject and source scope.
    Resolve {
        reference: ReferenceLocator,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        as_of_commit: Option<i64>,
    },
    /// Fill missing secondary addresses from the Namespace's authoritative records.
    Reconcile {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
impl ReferenceCommand {
    pub fn operation(self) -> Operation {
        Operation::Reference(match self {
            Self::Resolve {
                reference,
                subject,
                as_of_commit,
            } => ReferenceOperation::Resolve {
                subject_id: subject,
                reference,
                as_of_commit,
            },
            Self::Reconcile {
                namespace,
                operation_id,
            } => ReferenceOperation::Reconcile {
                namespace_id: namespace,
                operation_id: op_id(operation_id),
            },
        })
    }
}
