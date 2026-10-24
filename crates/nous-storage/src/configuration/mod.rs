//! Configuration history uses the same authority snapshot and operation as its consumers.
mod scopes;
use crate::{Database, ReadView, WriteTransaction, codec::*, storage_error, write_error};
use nous_domain::{ConfigEntry, ConfigScope, Error, NamespaceId, Result, SubjectId};
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};
use std::collections::BTreeSet;

impl ReadView {
    pub async fn configuration_scopes(&mut self) -> Result<Vec<ConfigScope>> {
        scopes::all(&mut self.tx).await
    }

    pub async fn validate_configuration_scope(&mut self, scope: &ConfigScope) -> Result<()> {
        scopes::validate(&mut self.tx, scope).await
    }

    pub async fn configuration_entries_all(&mut self) -> Result<Vec<ConfigEntry>> {
        latest_entries(&mut self.tx, None).await
    }
    pub async fn configuration_entries(
        &mut self,
        scopes: &[ConfigScope],
    ) -> Result<Vec<ConfigEntry>> {
        entries(&mut self.tx, scopes).await
    }

    pub async fn configuration_revision(&mut self) -> Result<i64> {
        revision(&mut self.tx).await
    }
}

impl WriteTransaction {
    pub async fn configuration_scopes(&mut self) -> Result<Vec<ConfigScope>> {
        scopes::all(&mut self.tx).await
    }

    pub async fn validate_configuration_scope(&mut self, scope: &ConfigScope) -> Result<()> {
        scopes::validate(&mut self.tx, scope).await
    }

    pub async fn configuration_entries_all(&mut self) -> Result<Vec<ConfigEntry>> {
        latest_entries(&mut self.tx, None).await
    }
    pub async fn configuration_entries(
        &mut self,
        scopes: &[ConfigScope],
    ) -> Result<Vec<ConfigEntry>> {
        entries(&mut self.tx, scopes).await
    }

    pub async fn configuration_revision(&mut self) -> Result<i64> {
        revision(&mut self.tx).await
    }

    pub async fn insert_configuration_change(&mut self, entry: &ConfigEntry) -> Result<()> {
        entry.validate()?;
        self.check_sequence(entry.commit_seq)?;
        self.validate_configuration_scope(&entry.scope).await?;
        let (kind, namespace, subject) = scope_columns(&entry.scope);
        sqlx::query("INSERT INTO configuration_changes(scope_kind,namespace_id,subject_id,key,revision,active,desired,apply_mode,recorded_at,commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(kind).bind(namespace).bind(subject).bind(&entry.key).bind(entry.revision)
            .bind(json_option(&entry.active)?).bind(json_option(&entry.desired)?)
            .bind(enum_name(&entry.apply_mode)?).bind(&entry.recorded_at).bind(entry.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}

impl Database {
    pub async fn configuration_entries_all(&self) -> Result<Vec<ConfigEntry>> {
        let mut view = self.read_view().await?;
        let entries = view.configuration_entries_all().await?;
        view.finish().await?;
        Ok(entries)
    }

    pub async fn configuration_history(
        &self,
        scope: &ConfigScope,
        key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConfigEntry>> {
        let limit = i64::try_from(limit)
            .ok()
            .filter(|limit| *limit > 0)
            .ok_or_else(|| Error::invalid("configuration history limit must be positive"))?;
        let (kind, namespace, subject) = scope_columns(scope);
        let mut view = self.read_view().await?;
        view.validate_configuration_scope(scope).await?;
        let entries = sqlx::query("SELECT * FROM configuration_changes WHERE scope_kind=? AND coalesce(namespace_id,'')=? AND coalesce(subject_id,'')=? AND (? IS NULL OR key=?) ORDER BY commit_seq DESC,key LIMIT ?")
            .bind(kind).bind(namespace.unwrap_or("")).bind(subject.unwrap_or(""))
            .bind(key).bind(key).bind(limit).fetch_all(&mut *view.tx).await.map_err(storage_error)?
            .into_iter().map(entry_row).collect::<Result<Vec<_>>>()?;
        view.finish().await?;
        Ok(entries)
    }
}

async fn entries(conn: &mut SqliteConnection, scopes: &[ConfigScope]) -> Result<Vec<ConfigEntry>> {
    let mut values = Vec::new();
    let mut visited = BTreeSet::new();
    for scope in scopes {
        if !visited.insert(scope) {
            continue;
        }
        scopes::validate(conn, scope).await?;
        values.extend(latest_entries(conn, Some(scope)).await?);
    }
    Ok(values)
}

async fn latest_entries(
    conn: &mut SqliteConnection,
    scope: Option<&ConfigScope>,
) -> Result<Vec<ConfigEntry>> {
    let columns = scope.map(scope_columns);
    sqlx::query("SELECT c.* FROM configuration_changes c WHERE (? IS NULL OR (c.scope_kind=? AND coalesce(c.namespace_id,'')=? AND coalesce(c.subject_id,'')=?)) AND NOT EXISTS(SELECT 1 FROM configuration_changes h WHERE h.scope_kind=c.scope_kind AND coalesce(h.namespace_id,'')=coalesce(c.namespace_id,'') AND coalesce(h.subject_id,'')=coalesce(c.subject_id,'') AND h.key=c.key AND h.commit_seq>c.commit_seq) ORDER BY c.scope_kind,c.namespace_id,c.subject_id,c.key")
        .bind(columns.map(|c|c.0)).bind(columns.map(|c|c.0))
        .bind(columns.and_then(|c|c.1).unwrap_or("")).bind(columns.and_then(|c|c.2).unwrap_or(""))
        .fetch_all(conn).await.map_err(storage_error)?.into_iter().map(entry_row).collect()
}

async fn revision(conn: &mut SqliteConnection) -> Result<i64> {
    sqlx::query_scalar("SELECT coalesce(max(commit_seq),0) FROM configuration_changes")
        .fetch_one(conn)
        .await
        .map_err(storage_error)
}

fn scope_columns(scope: &ConfigScope) -> (&'static str, Option<&str>, Option<&str>) {
    match scope {
        ConfigScope::Instance => ("instance", None, None),
        ConfigScope::Namespace { namespace_id } => ("namespace", Some(namespace_id.as_str()), None),
        ConfigScope::Subject {
            namespace_id,
            subject_id,
        } => (
            "subject",
            Some(namespace_id.as_str()),
            Some(subject_id.as_str()),
        ),
    }
}

fn scope_row(row: &SqliteRow) -> Result<ConfigScope> {
    Ok(match row.get::<&str, _>("scope_kind") {
        "instance" => ConfigScope::Instance,
        "namespace" => ConfigScope::Namespace {
            namespace_id: NamespaceId(row.get("namespace_id")),
        },
        "subject" => ConfigScope::Subject {
            namespace_id: NamespaceId(row.get("namespace_id")),
            subject_id: SubjectId(row.get("subject_id")),
        },
        _ => return Err(Error::internal("invalid persisted configuration scope")),
    })
}

fn entry_row(row: SqliteRow) -> Result<ConfigEntry> {
    Ok(ConfigEntry {
        scope: scope_row(&row)?,
        key: row.get("key"),
        revision: row.get("revision"),
        active: parse_optional(row.get("active"))?,
        desired: parse_optional(row.get("desired"))?,
        apply_mode: parse_enum(row.get("apply_mode"))?,
        recorded_at: row.get("recorded_at"),
        commit_seq: row.get("commit_seq"),
    })
}
