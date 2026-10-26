//! Every identity transport uses the same accepted operation and identity owner.
use crate::App;
use nous_domain::*;
use nous_foundation::identity::{AuthConfig, IssueGrant};
use nous_storage::{BeginOperation, OperationReceipt, operation_key};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityRequest {
    Whoami,
    List {
        after: Option<GrantId>,
        limit: usize,
    },
    Read {
        id: GrantId,
    },
    History {
        id: GrantId,
        limit: usize,
    },
    Issue {
        operation_id: String,
        request: IssueGrant,
    },
    Revoke {
        operation_id: String,
        id: GrantId,
        expected_revision: i64,
        reason: String,
    },
    Rotate {
        operation_id: String,
        id: GrantId,
        expected_revision: i64,
        reason: String,
    },
    Credential {
        reference: GrantRef,
    },
}

impl App {
    pub async fn authenticate(&self, token: &str) -> Result<CallerIdentity> {
        self.identity.authenticate(token).await
    }
    pub async fn bootstrap_credential(&self) -> Result<SecretRef> {
        Ok(self.database.grant(&GrantId::bootstrap()).await?.secret_ref)
    }
    pub async fn identity_request(
        &self,
        ctx: &CallContext,
        request: IdentityRequest,
    ) -> Result<Value> {
        match request {
            IdentityRequest::Whoami => Ok(json!(ctx.identity())),
            IdentityRequest::List { after, limit } => {
                Ok(json!(self.identity.list(ctx, after.as_ref(), limit).await?))
            }
            IdentityRequest::Read { id } => Ok(json!(self.identity.read(ctx, &id).await?)),
            IdentityRequest::History { id, limit } => {
                Ok(json!(self.identity.history(ctx, &id, limit).await?))
            }
            IdentityRequest::Credential { reference } => Ok(
                json!({"reference":reference,"token":self.identity.credential(ctx,&reference).await?.expose()}),
            ),
            IdentityRequest::Issue {
                operation_id,
                request,
            } => {
                let receipt = self.issue_grant(ctx, &operation_id, request).await?;
                self.deliver_credential(ctx, receipt).await
            }
            IdentityRequest::Rotate {
                operation_id,
                id,
                expected_revision,
                reason,
            } => {
                let receipt = self
                    .rotate_grant(ctx, &operation_id, &id, expected_revision, &reason)
                    .await?;
                self.deliver_credential(ctx, receipt).await
            }
            IdentityRequest::Revoke {
                operation_id,
                id,
                expected_revision,
                reason,
            } => Ok(json!(
                self.revoke_grant(ctx, &operation_id, &id, expected_revision, &reason)
                    .await?
            )),
        }
    }
    pub async fn issue_grant(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: IssueGrant,
    ) -> Result<OperationReceipt> {
        let prepared = self.identity.prepare_issue(ctx, request.clone()).await?;
        let scope = serde_json::to_string(&request.scope)?;
        let key = operation_key(ctx, "identity.issue", &scope, operation_id, &request)?;
        let mut operation = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(operation) => operation,
        };
        let grant = self.identity.issue_in(&mut operation, &prepared).await?;
        operation.commit(json!({"grant":grant})).await
    }

    async fn deliver_credential(
        &self,
        ctx: &CallContext,
        receipt: OperationReceipt,
    ) -> Result<Value> {
        let grant: AccessGrant = serde_json::from_value(receipt.result["grant"].clone())?;
        let value = self.identity.credential(ctx, &grant.reference()).await?;
        Ok(json!({"receipt":receipt,"token":value.expose()}))
    }
    pub async fn rotate_grant(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &GrantId,
        expected_revision: i64,
        reason: &str,
    ) -> Result<OperationReceipt> {
        let prepared = self.identity.prepare_rotation(ctx, id).await?;
        let key = operation_key(
            ctx,
            "identity.rotate",
            id.as_str(),
            operation_id,
            &json!({"id":id,"expected_revision":expected_revision,"reason":reason}),
        )?;
        let mut operation = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(operation) => operation,
        };
        let grant = self
            .identity
            .rotate_in(&mut operation, &prepared, expected_revision, reason)
            .await?;
        operation.commit(json!({"grant":grant})).await
    }
    pub async fn revoke_grant(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &GrantId,
        expected_revision: i64,
        reason: &str,
    ) -> Result<OperationReceipt> {
        self.identity.read(ctx, id).await?;
        let key = operation_key(
            ctx,
            "identity.revoke",
            id.as_str(),
            operation_id,
            &json!({"id":id,"expected_revision":expected_revision,"reason":reason}),
        )?;
        let mut operation = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(operation) => operation,
        };
        let grant = self
            .identity
            .revoke_in(&mut operation, id, expected_revision, reason)
            .await?;
        operation.commit(json!({"grant":grant})).await
    }
    /// A local instance owner resets all bearer credentials and publishes a new bootstrap seed.
    pub async fn reset_bootstrap(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        reason: &str,
    ) -> Result<OperationReceipt> {
        let prepared = self.identity.prepare_bootstrap_reset(ctx).await?;
        let key = operation_key(
            ctx,
            "identity.bootstrap.reset",
            "instance",
            operation_id,
            &json!({"reason":reason}),
        )?;
        let mut operation = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(operation) => operation,
        };
        let grant = self
            .identity
            .reset_bootstrap_in(&mut operation, &prepared, reason)
            .await?;
        let seed = nous_foundation::configuration::install_bootstrap_seed_in(
            &mut operation,
            &AuthConfig {
                admin_token: grant.secret_ref.clone(),
            },
        )
        .await?;
        operation.commit(json!({"grant":grant,"seed_revision":seed.revision,"issued_credentials_revoked":true})).await
    }
}
