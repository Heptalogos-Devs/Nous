//! Access-grant metadata has one immutable revision owner.
use crate::{
    Database, ReadView, WriteTransaction, codec::*, not_found, storage_error, write_error,
};
use nous_domain::*;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

impl Database {
    pub async fn grant(&self, id: &GrantId) -> Result<AccessGrant> {
        read(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            id,
            None,
        )
        .await
    }
    pub async fn grant_revision(&self, reference: &GrantRef) -> Result<AccessGrant> {
        read(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            &reference.id,
            Some(reference.revision),
        )
        .await
    }
    pub async fn grant_history(&self, id: &GrantId, limit: usize) -> Result<Vec<AccessGrant>> {
        let limit = positive_limit(limit)?;
        sqlx::query("SELECT * FROM grant_revisions WHERE grant_id=? ORDER BY revision DESC LIMIT ?")
            .bind(id.as_str())
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(row)
            .collect()
    }
    pub async fn grant_secret_referenced(&self, reference: &SecretRef) -> Result<bool> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM grant_revisions WHERE secret_ref=?)")
            .bind(serde_json::to_string(reference)?)
            .fetch_one(&self.pool)
            .await
            .map_err(storage_error)
    }
}

impl ReadView {
    pub async fn grant(&mut self, id: &GrantId) -> Result<AccessGrant> {
        read(&mut self.tx, id, None).await
    }
    pub async fn grant_revision(&mut self, reference: &GrantRef) -> Result<AccessGrant> {
        read(&mut self.tx, &reference.id, Some(reference.revision)).await
    }
    pub async fn grant_chain(&mut self, id: &GrantId) -> Result<Vec<AccessGrant>> {
        chain(&mut self.tx, id).await
    }
    pub async fn grants(
        &mut self,
        after: Option<&GrantId>,
        limit: usize,
    ) -> Result<Vec<AccessGrant>> {
        page(&mut self.tx, after, limit).await
    }
}

impl WriteTransaction {
    pub async fn grants(
        &mut self,
        after: Option<&GrantId>,
        limit: usize,
    ) -> Result<Vec<AccessGrant>> {
        page(&mut self.tx, after, limit).await
    }
    pub async fn grant(&mut self, id: &GrantId) -> Result<AccessGrant> {
        read(&mut self.tx, id, None).await
    }
    pub async fn grant_revision(&mut self, reference: &GrantRef) -> Result<AccessGrant> {
        read(&mut self.tx, &reference.id, Some(reference.revision)).await
    }
    pub async fn grant_chain(&mut self, id: &GrantId) -> Result<Vec<AccessGrant>> {
        chain(&mut self.tx, id).await
    }

    pub async fn save_grant(
        &mut self,
        grant: &AccessGrant,
        expected_revision: Option<i64>,
    ) -> Result<()> {
        grant.validate()?;
        self.check_sequence(grant.commit_seq)?;
        match &grant.scope {
            AccessScope::Admin => {}
            AccessScope::Namespace { namespace_id } => {
                self.namespace(namespace_id).await?;
            }
            AccessScope::Subject {
                namespace_id,
                subject_id,
            } => {
                self.subject(namespace_id, subject_id).await?;
            }
        }
        if let Some(expected) = expected_revision {
            let previous = self.grant(&grant.id).await?;
            if previous.revision != expected {
                return Err(Error::new(ErrorCode::Conflict, "grant revision changed"));
            }
            if previous.kind != grant.kind
                || previous.caller_id != grant.caller_id
                || previous.scope != grant.scope
                || previous.parent != grant.parent
                || previous.created_at != grant.created_at
            {
                return Err(Error::invalid(
                    "grant identity,scope,parent and creation time are immutable",
                ));
            }
        } else if let Some(parent) = &grant.parent {
            self.grant_revision(parent).await?;
            let parents = self.grant_chain(&parent.id).await?;
            if parents
                .iter()
                .any(|parent| !parent.active_at(self.time().unix_ms()))
            {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "delegating grant is revoked or expired",
                ));
            }
            let parent = parents.first().ok_or_else(not_found)?;
            if !parent.can_delegate {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "grant cannot delegate authority",
                ));
            }
            parent
                .identity()
                .begin_call(self.context().trace_id.clone())
                .narrow(grant.scope.clone())?;
            if parent.expires_at_unix_ms.is_some_and(|limit| {
                grant
                    .expires_at_unix_ms
                    .is_none_or(|expires| expires > limit)
            }) {
                return Err(Error::invalid("delegated grant outlives its parent"));
            }
        }
        if expected_revision.is_none() {
            sqlx::query("INSERT INTO access_grants(id,current_revision) VALUES(?,?)")
                .bind(grant.id.as_str())
                .bind(grant.revision)
                .execute(&mut *self.tx)
                .await
                .map_err(write_error)?;
        }
        let (scope, namespace, subject) = scope_columns(&grant.scope);
        sqlx::query("INSERT INTO grant_revisions(grant_id,revision,kind,name,caller_id,scope_kind,namespace_id,subject_id,parent_id,parent_revision,can_delegate,expires_at_unix_ms,state,credential_hash,secret_ref,created_at,recorded_at,actor,reason,commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(grant.id.as_str()).bind(grant.revision).bind(enum_name(&grant.kind)?).bind(&grant.name).bind(&grant.caller_id)
            .bind(scope).bind(namespace).bind(subject).bind(grant.parent.as_ref().map(|reference|reference.id.as_str())).bind(grant.parent.as_ref().map(|reference|reference.revision))
            .bind(grant.can_delegate).bind(grant.expires_at_unix_ms).bind(enum_name(&grant.state)?).bind(&grant.credential_hash).bind(serde_json::to_string(&grant.secret_ref)?)
            .bind(&grant.created_at).bind(&grant.recorded_at).bind(&grant.actor).bind(&grant.reason).bind(grant.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        if let Some(expected) = expected_revision {
            let changed = sqlx::query(
                "UPDATE access_grants SET current_revision=? WHERE id=? AND current_revision=?",
            )
            .bind(grant.revision)
            .bind(grant.id.as_str())
            .bind(expected)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?
            .rows_affected();
            if changed != 1 {
                return Err(Error::new(ErrorCode::Conflict, "grant revision changed"));
            }
        }
        Ok(())
    }
}

async fn read(
    conn: &mut SqliteConnection,
    id: &GrantId,
    revision: Option<i64>,
) -> Result<AccessGrant> {
    id.validate()?;
    row(sqlx::query("SELECT r.* FROM grant_revisions r JOIN access_grants h ON h.id=r.grant_id WHERE r.grant_id=? AND r.revision=coalesce(?,h.current_revision)")
        .bind(id.as_str()).bind(revision).fetch_optional(conn).await.map_err(storage_error)?.ok_or_else(not_found)?)
}

async fn chain(conn: &mut SqliteConnection, id: &GrantId) -> Result<Vec<AccessGrant>> {
    id.validate()?;
    let rows=sqlx::query("WITH RECURSIVE ancestors(id,depth) AS(SELECT ?,0 UNION ALL SELECT r.parent_id,a.depth+1 FROM ancestors a JOIN access_grants h ON h.id=a.id JOIN grant_revisions r ON r.grant_id=h.id AND r.revision=h.current_revision WHERE r.parent_id IS NOT NULL) SELECT r.* FROM ancestors a JOIN access_grants h ON h.id=a.id JOIN grant_revisions r ON r.grant_id=h.id AND r.revision=h.current_revision ORDER BY a.depth")
        .bind(id.as_str()).fetch_all(conn).await.map_err(storage_error)?;
    if rows.is_empty() {
        return Err(not_found());
    }
    rows.into_iter().map(row).collect()
}

fn scope_columns(scope: &AccessScope) -> (&'static str, Option<&str>, Option<&str>) {
    match scope {
        AccessScope::Admin => ("admin", None, None),
        AccessScope::Namespace { namespace_id } => ("namespace", Some(namespace_id.as_str()), None),
        AccessScope::Subject {
            namespace_id,
            subject_id,
        } => (
            "subject",
            Some(namespace_id.as_str()),
            Some(subject_id.as_str()),
        ),
    }
}
fn row(row: SqliteRow) -> Result<AccessGrant> {
    let scope = match row.get::<&str, _>("scope_kind") {
        "admin" => AccessScope::Admin,
        "namespace" => AccessScope::Namespace {
            namespace_id: NamespaceId(row.get("namespace_id")),
        },
        "subject" => AccessScope::Subject {
            namespace_id: NamespaceId(row.get("namespace_id")),
            subject_id: SubjectId(row.get("subject_id")),
        },
        _ => return Err(Error::internal("invalid persisted grant scope")),
    };
    Ok(AccessGrant {
        id: GrantId(row.get("grant_id")),
        kind: parse_enum(row.get("kind"))?,
        name: row.get("name"),
        caller_id: row.get("caller_id"),
        scope,
        parent: row
            .get::<Option<String>, _>("parent_id")
            .map(|id| GrantRef {
                id: GrantId(id),
                revision: row.get("parent_revision"),
            }),
        can_delegate: row.get("can_delegate"),
        expires_at_unix_ms: row.get("expires_at_unix_ms"),
        state: parse_enum(row.get("state"))?,
        credential_hash: row.get("credential_hash"),
        secret_ref: serde_json::from_str(row.get("secret_ref"))?,
        revision: row.get("revision"),
        created_at: row.get("created_at"),
        recorded_at: row.get("recorded_at"),
        actor: row.get("actor"),
        reason: row.get("reason"),
        commit_seq: row.get("commit_seq"),
    })
}
fn positive_limit(limit: usize) -> Result<i64> {
    i64::try_from(limit)
        .ok()
        .filter(|limit| *limit > 0)
        .ok_or_else(|| Error::invalid("grant limit must be positive"))
}

async fn page(
    conn: &mut SqliteConnection,
    after: Option<&GrantId>,
    limit: usize,
) -> Result<Vec<AccessGrant>> {
    sqlx::query("SELECT r.* FROM access_grants h JOIN grant_revisions r ON r.grant_id=h.id AND r.revision=h.current_revision WHERE (? IS NULL OR h.id>?) ORDER BY h.id LIMIT ?")
        .bind(after.map(GrantId::as_str)).bind(after.map(GrantId::as_str)).bind(positive_limit(limit)?)
        .fetch_all(conn).await.map_err(storage_error)?.into_iter().map(row).collect()
}
