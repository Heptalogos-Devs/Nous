//! Prompt history shares Authority snapshots, scope validation and compare-and-set operations.
use crate::{Database, ReadView, WriteTransaction, storage_error, write_error};
use nous_domain::{
    ConfigScope, Error, ErrorCode, PromptRef, PromptRevision, Result, validate_name,
};
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

impl Database {
    pub async fn prompt_revision(&self, reference: &PromptRef) -> Result<PromptRevision> {
        let mut view = self.read_view().await?;
        let result = view.prompt_revision(reference).await?;
        view.finish().await?;
        Ok(result)
    }
    pub async fn prompt_head(
        &self,
        scope: &ConfigScope,
        id: &str,
    ) -> Result<Option<PromptRevision>> {
        let mut view = self.read_view().await?;
        let result = view.prompt_head(scope, id).await?;
        view.finish().await?;
        Ok(result)
    }
    pub async fn prompt_history(
        &self,
        scope: &ConfigScope,
        id: &str,
        limit: usize,
    ) -> Result<Vec<PromptRevision>> {
        let mut view = self.read_view().await?;
        let result = view.prompt_history(scope, id, limit).await?;
        view.finish().await?;
        Ok(result)
    }
    pub async fn prompt_catalog(
        &self,
        scopes: &[ConfigScope],
        limit: usize,
    ) -> Result<Vec<PromptRevision>> {
        let mut view = self.read_view().await?;
        let result = view.prompt_catalog(scopes, limit).await?;
        view.finish().await?;
        Ok(result)
    }
}
impl ReadView {
    pub async fn prompt_revision(&mut self, reference: &PromptRef) -> Result<PromptRevision> {
        reference.validate()?;
        self.validate_configuration_scope(&reference.scope).await?;
        exact(&mut self.tx, reference).await
    }
    pub async fn prompt_head(
        &mut self,
        scope: &ConfigScope,
        id: &str,
    ) -> Result<Option<PromptRevision>> {
        self.validate_configuration_scope(scope).await?;
        head(&mut self.tx, scope, id).await
    }
    pub async fn prompt_history(
        &mut self,
        scope: &ConfigScope,
        id: &str,
        limit: usize,
    ) -> Result<Vec<PromptRevision>> {
        self.validate_configuration_scope(scope).await?;
        history(&mut self.tx, scope, id, limit).await
    }
    pub async fn prompt_catalog(
        &mut self,
        scopes: &[ConfigScope],
        limit: usize,
    ) -> Result<Vec<PromptRevision>> {
        for scope in scopes {
            self.validate_configuration_scope(scope).await?;
        }
        catalog(&mut self.tx, scopes, limit).await
    }
}
impl WriteTransaction {
    pub async fn prompt_revision(&mut self, reference: &PromptRef) -> Result<PromptRevision> {
        reference.validate()?;
        self.validate_configuration_scope(&reference.scope).await?;
        exact(&mut self.tx, reference).await
    }
    pub async fn prompt_head(
        &mut self,
        scope: &ConfigScope,
        id: &str,
    ) -> Result<Option<PromptRevision>> {
        self.validate_configuration_scope(scope).await?;
        head(&mut self.tx, scope, id).await
    }
    pub async fn prompt_history(
        &mut self,
        scope: &ConfigScope,
        id: &str,
        limit: usize,
    ) -> Result<Vec<PromptRevision>> {
        self.validate_configuration_scope(scope).await?;
        history(&mut self.tx, scope, id, limit).await
    }
    pub async fn prompt_catalog(
        &mut self,
        scopes: &[ConfigScope],
        limit: usize,
    ) -> Result<Vec<PromptRevision>> {
        for scope in scopes {
            self.validate_configuration_scope(scope).await?;
        }
        catalog(&mut self.tx, scopes, limit).await
    }
    pub async fn insert_prompt_revision(
        &mut self,
        revision: &PromptRevision,
        expected_revision: i64,
    ) -> Result<()> {
        revision.validate()?;
        self.check_sequence(revision.reference.revision)?;
        self.validate_configuration_scope(&revision.reference.scope)
            .await?;
        let current = head(
            &mut self.tx,
            &revision.reference.scope,
            &revision.reference.id,
        )
        .await?;
        if expected_revision < 0
            || current.as_ref().map_or(0, |p| p.reference.revision) != expected_revision
        {
            return Err(Error::new(ErrorCode::Conflict, "prompt revision changed"));
        }
        sqlx::query("INSERT INTO prompt_revisions(scope,id,revision,locale,body,author,reason,recorded_at) VALUES(?,?,?,?,?,?,?,?)")
            .bind(serde_json::to_string(&revision.reference.scope)?).bind(&revision.reference.id).bind(revision.reference.revision)
            .bind(&revision.locale).bind(&revision.body).bind(&revision.author).bind(&revision.reason).bind(&revision.recorded_at)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
async fn exact(conn: &mut SqliteConnection, reference: &PromptRef) -> Result<PromptRevision> {
    let row = sqlx::query("SELECT * FROM prompt_revisions WHERE scope=? AND id=? AND revision=?")
        .bind(serde_json::to_string(&reference.scope)?)
        .bind(&reference.id)
        .bind(reference.revision)
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| Error::new(ErrorCode::NotFound, "prompt revision was not found"))?;
    decode(row)
}
async fn head(
    conn: &mut SqliteConnection,
    scope: &ConfigScope,
    id: &str,
) -> Result<Option<PromptRevision>> {
    validate_name(id, "prompt id")?;
    sqlx::query(
        "SELECT * FROM prompt_revisions WHERE scope=? AND id=? ORDER BY revision DESC LIMIT 1",
    )
    .bind(serde_json::to_string(scope)?)
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(storage_error)?
    .map(decode)
    .transpose()
}
async fn history(
    conn: &mut SqliteConnection,
    scope: &ConfigScope,
    id: &str,
    limit: usize,
) -> Result<Vec<PromptRevision>> {
    validate_name(id, "prompt id")?;
    sqlx::query(
        "SELECT * FROM prompt_revisions WHERE scope=? AND id=? ORDER BY revision DESC LIMIT ?",
    )
    .bind(serde_json::to_string(scope)?)
    .bind(id)
    .bind(bound(limit)?)
    .fetch_all(conn)
    .await
    .map_err(storage_error)?
    .into_iter()
    .map(decode)
    .collect()
}
async fn catalog(
    conn: &mut SqliteConnection,
    scopes: &[ConfigScope],
    limit: usize,
) -> Result<Vec<PromptRevision>> {
    if scopes.is_empty() || scopes.len() > 3 {
        return Err(Error::invalid("prompt catalog needs 1..3 scope levels"));
    }
    let limit = bound(limit)?;
    let mut result = Vec::new();
    for scope in scopes {
        let rows=sqlx::query("SELECT p.* FROM prompt_revisions p WHERE p.scope=? AND NOT EXISTS(SELECT 1 FROM prompt_revisions n WHERE n.scope=p.scope AND n.id=p.id AND n.revision>p.revision) ORDER BY p.id LIMIT ?")
            .bind(serde_json::to_string(scope)?).bind(limit).fetch_all(&mut *conn).await.map_err(storage_error)?;
        result.extend(rows.into_iter().map(decode).collect::<Result<Vec<_>>>()?);
    }
    result.sort_by(|a, b| {
        a.reference
            .id
            .cmp(&b.reference.id)
            .then(a.reference.scope.cmp(&b.reference.scope))
    });
    result.truncate(limit as usize);
    Ok(result)
}
fn bound(limit: usize) -> Result<i64> {
    if !(1..=1001).contains(&limit) {
        return Err(Error::invalid("prompt limit must be 1..1000"));
    }
    Ok(limit as i64)
}
fn decode(row: SqliteRow) -> Result<PromptRevision> {
    Ok(PromptRevision {
        reference: PromptRef {
            scope: serde_json::from_str(row.get("scope"))?,
            id: row.get("id"),
            revision: row.get("revision"),
        },
        locale: row.get("locale"),
        body: row.get("body"),
        author: row.get("author"),
        reason: row.get("reason"),
        recorded_at: row.get("recorded_at"),
    })
}
