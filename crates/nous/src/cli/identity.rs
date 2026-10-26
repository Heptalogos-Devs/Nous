//! Identity flags describe authority explicitly; online and MCP use the same typed requests.
use super::op_id;
use crate::http::Operation;
use anyhow::{Result, bail};
use clap::Subcommand;
use nous_app::administration::identity::IdentityRequest;
use nous_domain::{AccessScope, GrantId, GrantRef, NamespaceId, SubjectId};
use nous_foundation::identity::IssueGrant;

#[derive(Subcommand)]
pub(super) enum IdentityCommand {
    Whoami,
    List {
        #[arg(long)]
        after: Option<GrantId>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Get {
        id: GrantId,
    },
    History {
        id: GrantId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Issue {
        #[arg(long)]
        name: String,
        #[arg(
            long,
            required_unless_present = "namespace",
            conflicts_with = "namespace"
        )]
        admin: bool,
        #[arg(long)]
        namespace: Option<NamespaceId>,
        #[arg(long, requires = "namespace")]
        subject: Option<SubjectId>,
        #[arg(long)]
        can_delegate: bool,
        #[arg(long)]
        expires_at_unix_ms: Option<i64>,
        #[arg(long, default_value = "credential issued")]
        reason: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Rotate {
        id: GrantId,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long, default_value = "credential rotated")]
        reason: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Revoke {
        id: GrantId,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long, default_value = "credential revoked")]
        reason: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Explicitly read a credential owned by this caller or a delegating ancestor.
    Credential {
        id: GrantId,
        #[arg(long)]
        revision: i64,
    },
    /// Reset the stopped instance administrator and revoke all issued bearer credentials.
    ResetBootstrap {
        #[arg(long, default_value = "local administrator recovery")]
        reason: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
}

impl IdentityCommand {
    pub(super) fn operation(self) -> Result<Operation> {
        let request = match self {
            Self::Whoami => IdentityRequest::Whoami,
            Self::List { after, limit } => IdentityRequest::List { after, limit },
            Self::Get { id } => IdentityRequest::Read { id },
            Self::History { id, limit } => IdentityRequest::History { id, limit },
            Self::Issue {
                name,
                admin,
                namespace,
                subject,
                can_delegate,
                expires_at_unix_ms,
                reason,
                operation_id,
            } => {
                let scope = match (namespace, subject) {
                    (Some(namespace_id), Some(subject_id)) => AccessScope::Subject {
                        namespace_id,
                        subject_id,
                    },
                    (Some(namespace_id), None) => AccessScope::Namespace { namespace_id },
                    (None, None) if admin => AccessScope::Admin,
                    _ => bail!(
                        "grant issue requires --admin or --namespace; --subject requires --namespace"
                    ),
                };
                IdentityRequest::Issue {
                    operation_id: op_id(operation_id),
                    request: IssueGrant {
                        name,
                        scope,
                        can_delegate,
                        expires_at_unix_ms,
                        reason,
                    },
                }
            }
            Self::Rotate {
                id,
                expected_revision,
                reason,
                operation_id,
            } => IdentityRequest::Rotate {
                operation_id: op_id(operation_id),
                id,
                expected_revision,
                reason,
            },
            Self::Revoke {
                id,
                expected_revision,
                reason,
                operation_id,
            } => IdentityRequest::Revoke {
                operation_id: op_id(operation_id),
                id,
                expected_revision,
                reason,
            },
            Self::Credential { id, revision } => IdentityRequest::Credential {
                reference: GrantRef { id, revision },
            },
            Self::ResetBootstrap { .. } => {
                bail!("bootstrap reset is a local instance-owner operation")
            }
        };
        Ok(Operation::Identity(request))
    }
}
