//! Shared CLI selection of a configuration/model scope.
use clap::Args;
use nous_domain::{ConfigScope, NamespaceId, SubjectId};

#[derive(Default, Args)]
pub struct ScopeArgs {
    #[arg(long)]
    namespace: Option<NamespaceId>,
    #[arg(long, requires = "namespace")]
    subject: Option<SubjectId>,
}
impl ScopeArgs {
    pub(super) fn scope(self) -> Option<ConfigScope> {
        match (self.namespace, self.subject) {
            (Some(namespace_id), Some(subject_id)) => Some(ConfigScope::Subject {
                namespace_id,
                subject_id,
            }),
            (Some(namespace_id), None) => Some(ConfigScope::Namespace { namespace_id }),
            _ => None,
        }
    }
}
