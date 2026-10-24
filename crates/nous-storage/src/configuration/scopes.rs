//! Real configuration scopes, including owners with no persisted overrides.
use super::scope_row;
use crate::{authority::records, not_found, storage_error};
use nous_domain::{ConfigScope, Result};
use sqlx::SqliteConnection;

pub(super) async fn all(conn: &mut SqliteConnection) -> Result<Vec<ConfigScope>> {
    sqlx::query("SELECT 'instance' AS scope_kind,NULL AS namespace_id,NULL AS subject_id UNION ALL SELECT 'namespace',id,NULL FROM namespaces UNION ALL SELECT 'subject',namespace_id,id FROM subjects ORDER BY scope_kind,namespace_id,subject_id")
        .fetch_all(conn).await.map_err(storage_error)?.into_iter().map(|row|scope_row(&row)).collect()
}

pub(super) async fn validate(conn: &mut SqliteConnection, scope: &ConfigScope) -> Result<()> {
    match scope {
        ConfigScope::Instance => {}
        ConfigScope::Namespace { namespace_id } => {
            records::namespace(conn, namespace_id)
                .await?
                .ok_or_else(not_found)?;
        }
        ConfigScope::Subject {
            namespace_id,
            subject_id,
        } => {
            records::subject(conn, namespace_id, subject_id)
                .await?
                .ok_or_else(not_found)?;
        }
    }
    Ok(())
}
