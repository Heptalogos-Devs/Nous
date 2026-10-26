//! Live grants own authentication and delegation; accepted jobs retain their own lifecycle.
mod config;
mod requests;
use crate::{
    clock::Clock,
    secret::{PreparedCredential, SecretValue, Secrets},
};
pub use config::AuthConfig;
use nous_domain::*;
use nous_storage::{BeginOperation, Database, WriteTransaction, operation_key};
pub use requests::{GrantPage, IssueGrant};
use serde_json::json;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct Identity {
    database: Database,
    secrets: Secrets,
    clock: Clock,
}
pub struct PreparedIssue {
    id: GrantId,
    request: IssueGrant,
    credential: PreparedCredential,
}
pub struct PreparedRotation {
    id: GrantId,
    credential: PreparedCredential,
}

impl Identity {
    pub fn new(database: Database, secrets: Secrets, clock: Clock) -> Self {
        Self {
            database,
            secrets,
            clock,
        }
    }
    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    pub async fn authenticate(&self, token: &str) -> Result<CallerIdentity> {
        if token.trim() != token
            || token.is_empty()
            || token.len() > 4096
            || token.bytes().any(|byte| byte.is_ascii_whitespace())
        {
            return Err(unauthorized());
        }
        let value = SecretValue::parse(token).map_err(|_| unauthorized())?;
        let candidate = token
            .split_once('.')
            .and_then(|(id, _)| id.parse::<GrantId>().ok());
        let mut ids = vec![GrantId::bootstrap()];
        if let Some(id) = candidate
            && id != GrantId::bootstrap()
        {
            ids.insert(0, id);
        }
        let digest = Secrets::digest(&value);
        let mut view = self.database.read_view().await?;
        let now = self.clock.utc().timestamp_millis();
        for id in ids {
            let chain = match view.grant_chain(&id).await {
                Ok(chain) => chain,
                Err(error) if error.code == ErrorCode::NotFound => continue,
                Err(error) => return Err(error),
            };
            let current = chain.first().ok_or_else(unauthorized)?;
            if chain.iter().all(|grant| grant.active_at(now))
                && bool::from(current.credential_hash.as_bytes().ct_eq(digest.as_bytes()))
            {
                let identity = current.identity();
                view.finish().await?;
                return Ok(identity);
            }
        }
        Err(unauthorized())
    }

    pub async fn prepare_issue(
        &self,
        ctx: &CallContext,
        request: IssueGrant,
    ) -> Result<PreparedIssue> {
        validate_name(&request.name, "grant name")?;
        validate_name(&request.reason, "grant reason")?;
        ctx.narrow(request.scope.clone())?;
        let mut view = self.database.read_view().await?;
        if let Some(authorization) = &ctx.authorization {
            let chain = view.grant_chain(&authorization.id).await?;
            let actor = check_actor(ctx, &chain, self.clock.utc().timestamp_millis())?;
            if !actor.can_delegate {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "grant cannot delegate authority",
                ));
            }
        } else {
            ctx.require_admin()?;
        }
        view.finish().await?;
        let id = GrantId::new();
        let credential = self
            .secrets
            .prepare_access(self.database.clone(), &id)
            .await?;
        Ok(PreparedIssue {
            id,
            request,
            credential,
        })
    }

    pub async fn issue_in(
        &self,
        operation: &mut WriteTransaction,
        prepared: &PreparedIssue,
    ) -> Result<AccessGrant> {
        let context = operation.context().clone();
        let ctx = &context;
        ctx.narrow(prepared.request.scope.clone())?;
        let parent = if let Some(authorization) = &ctx.authorization {
            let chain = operation.grant_chain(&authorization.id).await?;
            let actor = check_actor(ctx, &chain, operation.time().unix_ms())?;
            if !actor.can_delegate {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "grant cannot delegate authority",
                ));
            }
            Some(actor.clone())
        } else {
            ctx.require_admin()?;
            None
        };
        let expires = prepared
            .request
            .expires_at_unix_ms
            .or_else(|| parent.as_ref().and_then(|parent| parent.expires_at_unix_ms));
        if expires.is_some_and(|expiry| expiry <= operation.time().unix_ms()) {
            return Err(Error::invalid("new grant expiry must be in the future"));
        }
        let recorded = operation.time().recorded();
        let grant = AccessGrant {
            id: prepared.id.clone(),
            kind: GrantKind::Issued,
            name: prepared.request.name.clone(),
            caller_id: format!("grant:{}", prepared.id),
            scope: prepared.request.scope.clone(),
            parent: parent.as_ref().map(AccessGrant::reference),
            can_delegate: prepared.request.can_delegate,
            expires_at_unix_ms: expires,
            state: GrantState::Active,
            credential_hash: prepared.credential.digest.clone(),
            secret_ref: prepared.credential.reference.clone(),
            revision: operation.commit_seq(),
            created_at: recorded.clone(),
            recorded_at: recorded,
            actor: ctx.caller_id.clone(),
            reason: prepared.request.reason.clone(),
            commit_seq: operation.commit_seq(),
        };
        prepared.credential.protect_in(operation);
        operation.save_grant(&grant, None).await?;
        Ok(grant)
    }

    pub async fn prepare_rotation(
        &self,
        ctx: &CallContext,
        id: &GrantId,
    ) -> Result<PreparedRotation> {
        let grant = self.read(ctx, id).await?;
        if grant.kind == GrantKind::Bootstrap {
            return Err(Error::invalid(
                "bootstrap credential changes use the offline reset owner",
            ));
        }
        let credential = self
            .secrets
            .prepare_access(self.database.clone(), id)
            .await?;
        Ok(PreparedRotation {
            id: id.clone(),
            credential,
        })
    }

    pub async fn rotate_in(
        &self,
        operation: &mut WriteTransaction,
        prepared: &PreparedRotation,
        expected_revision: i64,
        reason: &str,
    ) -> Result<AccessGrant> {
        let context = operation.context().clone();
        let ctx = &context;
        validate_name(reason, "grant rotation reason")?;
        let mut grant = operation.grant(&prepared.id).await?;
        authorize_in(ctx, operation, &grant).await?;
        if grant.kind == GrantKind::Bootstrap {
            return Err(Error::invalid(
                "bootstrap credential changes use the offline reset owner",
            ));
        }
        if !grant.active_at(operation.time().unix_ms()) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "revoked or expired grant cannot rotate",
            ));
        }
        grant.credential_hash = prepared.credential.digest.clone();
        grant.secret_ref = prepared.credential.reference.clone();
        stamp(&mut grant, ctx, operation, reason);
        prepared.credential.protect_in(operation);
        operation
            .save_grant(&grant, Some(expected_revision))
            .await?;
        Ok(grant)
    }

    pub async fn revoke_in(
        &self,
        operation: &mut WriteTransaction,
        id: &GrantId,
        expected_revision: i64,
        reason: &str,
    ) -> Result<AccessGrant> {
        let context = operation.context().clone();
        let ctx = &context;
        validate_name(reason, "grant revocation reason")?;
        let mut grant = operation.grant(id).await?;
        authorize_in(ctx, operation, &grant).await?;
        if grant.revision != expected_revision {
            return Err(Error::new(ErrorCode::Conflict, "grant revision changed"));
        }
        if grant.state == GrantState::Revoked {
            return Ok(grant);
        }
        grant.state = GrantState::Revoked;
        stamp(&mut grant, ctx, operation, reason);
        operation
            .save_grant(&grant, Some(expected_revision))
            .await?;
        Ok(grant)
    }

    pub async fn read(&self, ctx: &CallContext, id: &GrantId) -> Result<AccessGrant> {
        let mut view = self.database.read_view().await?;
        let target = view.grant_chain(id).await?;
        authorize_target(ctx, &target)?;
        let grant = target.into_iter().next().ok_or_else(unauthorized)?;
        view.finish().await?;
        Ok(grant)
    }
    pub async fn history(
        &self,
        ctx: &CallContext,
        id: &GrantId,
        limit: usize,
    ) -> Result<Vec<AccessGrant>> {
        self.read(ctx, id).await?;
        self.database.grant_history(id, limit).await
    }
    pub async fn list(
        &self,
        ctx: &CallContext,
        after: Option<&GrantId>,
        limit: usize,
    ) -> Result<GrantPage> {
        ctx.require_admin()?;
        if !(1..=1000).contains(&limit) {
            return Err(Error::invalid("grant page limit must be 1..1000"));
        }
        let mut view = self.database.read_view().await?;
        let mut grants = view.grants(after, limit + 1).await?;
        let has_more = grants.len() > limit;
        grants.truncate(limit);
        let next = grants.last().map(|grant| grant.id.clone());
        view.finish().await?;
        Ok(GrantPage {
            grants,
            next,
            has_more,
        })
    }
    pub async fn credential(&self, ctx: &CallContext, reference: &GrantRef) -> Result<SecretValue> {
        self.read(ctx, &reference.id).await?;
        let exact = self.database.grant_revision(reference).await?;
        self.secrets.read(&exact.secret_ref).await
    }

    pub async fn initialize_bootstrap(&self, reference: &SecretRef) -> Result<AccessGrant> {
        match self.database.grant(&GrantId::bootstrap()).await {
            Ok(grant) => return Ok(grant),
            Err(error) if error.code == ErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        let value = self.secrets.read(reference).await?;
        if value.expose().len() < 32 {
            return Err(Error::invalid(
                "bootstrap credential must contain at least 32 bytes",
            ));
        }
        let ctx = CallContext::new("runtime.bootstrap", AccessScope::Admin);
        let key = operation_key(
            &ctx,
            "identity.bootstrap",
            "instance",
            "initialize",
            &json!({"reference":reference,"digest":Secrets::digest(&value)}),
        )?;
        let mut operation = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(_) => return self.database.grant(&GrantId::bootstrap()).await,
            BeginOperation::New(operation) => operation,
        };
        let grant = bootstrap(
            &ctx,
            &operation,
            reference.clone(),
            Secrets::digest(&value),
            None,
            "initial administrator",
        );
        operation.save_grant(&grant, None).await?;
        operation.commit(json!({"grant":grant.reference()})).await?;
        Ok(grant)
    }

    pub async fn prepare_bootstrap_reset(&self, ctx: &CallContext) -> Result<PreparedCredential> {
        offline_administrator(ctx)?;
        self.secrets
            .prepare_access(self.database.clone(), &GrantId::bootstrap())
            .await
    }
    pub async fn reset_bootstrap_in(
        &self,
        operation: &mut WriteTransaction,
        credential: &PreparedCredential,
        reason: &str,
    ) -> Result<AccessGrant> {
        let context = operation.context().clone();
        let ctx = &context;
        offline_administrator(ctx)?;
        validate_name(reason, "bootstrap reset reason")?;
        self.revoke_issued_in(ctx, operation, reason).await?;
        let previous = match operation.grant(&GrantId::bootstrap()).await {
            Ok(grant) => Some(grant),
            Err(error) if error.code == ErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        let grant = bootstrap(
            ctx,
            operation,
            credential.reference.clone(),
            credential.digest.clone(),
            previous.as_ref().map(|grant| grant.created_at.clone()),
            reason,
        );
        credential.protect_in(operation);
        operation
            .save_grant(&grant, previous.as_ref().map(|grant| grant.revision))
            .await?;
        Ok(grant)
    }

    pub async fn reset_restored_in(
        &self,
        operation: &mut WriteTransaction,
        reference: &SecretRef,
        digest: String,
    ) -> Result<AccessGrant> {
        let context = operation.context().clone();
        let ctx = &context;
        offline_administrator(ctx)?;
        self.revoke_issued_in(ctx, operation, "restored instance uses new credentials")
            .await?;
        let previous = match operation.grant(&GrantId::bootstrap()).await {
            Ok(grant) => Some(grant),
            Err(error) if error.code == ErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        let grant = bootstrap(
            ctx,
            operation,
            reference.clone(),
            digest,
            previous.as_ref().map(|grant| grant.created_at.clone()),
            "restored administrator",
        );
        operation
            .save_grant(&grant, previous.as_ref().map(|grant| grant.revision))
            .await?;
        Ok(grant)
    }

    async fn revoke_issued_in(
        &self,
        ctx: &CallContext,
        operation: &mut WriteTransaction,
        reason: &str,
    ) -> Result<()> {
        let mut after = None;
        loop {
            let grants = operation.grants(after.as_ref(), 256).await?;
            if grants.is_empty() {
                break;
            }
            after = grants.last().map(|grant| grant.id.clone());
            for mut grant in grants {
                if grant.kind == GrantKind::Bootstrap || grant.state == GrantState::Revoked {
                    continue;
                }
                let previous = grant.revision;
                grant.state = GrantState::Revoked;
                stamp(&mut grant, ctx, operation, reason);
                operation.save_grant(&grant, Some(previous)).await?;
            }
        }
        Ok(())
    }
}

fn unauthorized() -> Error {
    Error::new(
        ErrorCode::Unauthorized,
        "a current unexpired bearer grant is required",
    )
}
fn offline_administrator(ctx: &CallContext) -> Result<()> {
    ctx.require_admin()?;
    if ctx.authorization.is_some() {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "bootstrap reset requires the local instance owner",
        ));
    }
    Ok(())
}
fn check_actor<'a>(
    ctx: &CallContext,
    chain: &'a [AccessGrant],
    now: i64,
) -> Result<&'a AccessGrant> {
    let actor = chain.first().ok_or_else(unauthorized)?;
    if actor.caller_id != ctx.caller_id || chain.iter().any(|grant| !grant.active_at(now)) {
        return Err(unauthorized());
    }
    actor
        .identity()
        .begin_call(ctx.trace_id.clone())
        .narrow(ctx.scope.clone())?;
    Ok(actor)
}
fn authorize_target(ctx: &CallContext, chain: &[AccessGrant]) -> Result<()> {
    let target = chain.first().ok_or_else(unauthorized)?;
    ctx.narrow(target.scope.clone())?;
    if matches!(ctx.scope, AccessScope::Admin) {
        return Ok(());
    }
    let actor = ctx.authorization.as_ref().ok_or_else(unauthorized)?;
    if !chain.iter().any(|grant| grant.id == actor.id) {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "grant management requires its owner or delegating ancestor",
        ));
    }
    Ok(())
}
async fn authorize_in(
    ctx: &CallContext,
    operation: &mut WriteTransaction,
    target: &AccessGrant,
) -> Result<()> {
    let chain = operation.grant_chain(&target.id).await?;
    authorize_target(ctx, &chain)
}
fn stamp(grant: &mut AccessGrant, ctx: &CallContext, operation: &WriteTransaction, reason: &str) {
    grant.revision = operation.commit_seq();
    grant.commit_seq = operation.commit_seq();
    grant.recorded_at = operation.time().recorded();
    grant.actor = ctx.caller_id.clone();
    grant.reason = reason.into();
}
fn bootstrap(
    ctx: &CallContext,
    operation: &WriteTransaction,
    reference: SecretRef,
    digest: String,
    created_at: Option<String>,
    reason: &str,
) -> AccessGrant {
    let recorded = operation.time().recorded();
    AccessGrant {
        id: GrantId::bootstrap(),
        kind: GrantKind::Bootstrap,
        name: "administrator".into(),
        caller_id: "admin".into(),
        scope: AccessScope::Admin,
        parent: None,
        can_delegate: true,
        expires_at_unix_ms: None,
        state: GrantState::Active,
        credential_hash: digest,
        secret_ref: reference,
        revision: operation.commit_seq(),
        created_at: created_at.unwrap_or_else(|| recorded.clone()),
        recorded_at: recorded,
        actor: ctx.caller_id.clone(),
        reason: reason.into(),
        commit_seq: operation.commit_seq(),
    }
}
