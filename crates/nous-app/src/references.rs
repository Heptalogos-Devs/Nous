//! Readable address use and explicit directory reconciliation share the authority owners.
use crate::App;
use nous_domain::{references::*, *};
use nous_storage::{BeginOperation, OperationReceipt, operation_key};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReferenceOperation {
    Resolve {
        subject_id: SubjectId,
        reference: ReferenceLocator,
        as_of_commit: Option<i64>,
    },
    Reconcile {
        namespace_id: NamespaceId,
        operation_id: String,
    },
}
impl App {
    pub async fn reference_operation(
        &self,
        ctx: &CallContext,
        request: ReferenceOperation,
    ) -> Result<Value> {
        Ok(match request {
            ReferenceOperation::Resolve {
                subject_id,
                reference,
                as_of_commit,
            } => json!(
                self.resolve_reference(ctx, &subject_id, &reference, as_of_commit)
                    .await?
            ),
            ReferenceOperation::Reconcile {
                namespace_id,
                operation_id,
            } => json!(
                self.reconcile_references(ctx, &operation_id, &namespace_id)
                    .await?
            ),
        })
    }
    pub async fn reconcile_references(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        namespace: &NamespaceId,
    ) -> Result<OperationReceipt> {
        ctx.require_namespace(namespace)?;
        let key = operation_key(
            ctx,
            "references.reconcile",
            &format!("namespace:{namespace}"),
            operation_id,
            &json!({"namespace_id":namespace}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let targets = tx.reconcile_reference_addresses(namespace).await?;
        tx.commit(json!({"namespace_id":namespace,"targets":targets}))
            .await
    }
    pub async fn resolve_reference(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        locator: &ReferenceLocator,
        as_of: Option<i64>,
    ) -> Result<ReferenceResolution> {
        let namespace = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&namespace, subject)?;
        let mut view = self.database.read_view().await?;
        let source_read = view.source_read_view(ctx, &namespace, subject).await?;
        let reference = match locator {
            ReferenceLocator::Canonical(reference) => reference.clone(),
            ReferenceLocator::Readable(address) => {
                view.resolve_reference_locator(
                    &namespace,
                    subject,
                    address.as_str(),
                    &nous_storage::ReferenceViewOptions {
                        as_of,
                        source_read: Some(&source_read),
                    },
                )
                .await?
            }
        };
        let filters = QueryFilters::default();
        let options = nous_storage::QuerySnapshotOptions {
            space_id: "",
            as_of_commit: as_of,
            limit: 1,
            source_read: Some(&source_read),
            filters: &filters,
            history: false,
        };
        view.require_retrieval_reference(&namespace, subject, &options, &reference)
            .await?;
        let addresses = view.retrieval_addresses(&namespace, &reference).await?;
        view.finish().await?;
        Ok(ReferenceResolution {
            namespace_id: namespace,
            subject_id: subject.clone(),
            reference,
            addresses,
        })
    }
}
