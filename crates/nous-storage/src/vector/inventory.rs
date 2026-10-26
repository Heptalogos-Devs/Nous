use crate::{SourceReadView, storage_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection};

/// An Authority read projection; the application's QueryRequest remains the query authority.
pub struct QuerySnapshotOptions<'a> {
    pub space_id: &'a str,
    pub as_of_commit: Option<i64>,
    pub limit: usize,
    pub source_read: Option<&'a SourceReadView>,
    pub filters: &'a QueryFilters,
    pub history: bool,
}
pub(super) async fn permitted(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    options: &QuerySnapshotOptions<'_>,
    exact: Option<&CognitiveRef>,
) -> Result<Option<String>> {
    let scoped = options.source_read.filter(|scope| scope.is_restricted());
    if scoped.is_none() && options.filters.is_empty() {
        return Ok(None);
    }
    if let Some(reference) = exact {
        if let Some(scope) = scoped
            && !crate::source::read_scope::target_allowed(
                conn,
                scope,
                &RetrievalTarget::Cognition(reference.clone()),
            )
            .await?
        {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "exact cognition reference is outside the source-read scope",
            ));
        }
        let matches = crate::source::query_filter::target_matches(
            conn,
            ns,
            subject,
            &RetrievalTarget::Cognition(reference.clone()),
            options.filters,
            options.as_of_commit,
        )
        .await?;
        return Ok(Some(serde_json::to_string(&if matches {
            vec![reference.to_string()]
        } else {
            Vec::new()
        })?));
    }
    if !options.history && options.filters.is_empty() {
        return crate::source::read_scope::candidates::cognitive_keys(
            conn,
            ns,
            subject,
            options.as_of_commit,
            scoped.expect("restricted scope"),
        )
        .await
        .map(Some);
    }
    let references = references(conn, ns, subject, options.as_of_commit, options.history).await?;
    let mut keys = Vec::new();
    for reference in references {
        let target = RetrievalTarget::Cognition(reference.clone());
        if let Some(scope) = scoped
            && !crate::source::read_scope::target_allowed(conn, scope, &target).await?
        {
            continue;
        }
        if crate::source::query_filter::target_matches(
            conn,
            ns,
            subject,
            &target,
            options.filters,
            options.as_of_commit,
        )
        .await?
        {
            keys.push(reference.to_string());
        }
    }
    Ok(Some(serde_json::to_string(&keys)?))
}

pub(crate) async fn references(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    as_of: Option<i64>,
    history: bool,
) -> Result<Vec<CognitiveRef>> {
    let rows=sqlx::query("SELECT 'memory' AS kind,m.id,r.revision FROM memories m JOIN memory_revisions r ON r.memory_id=m.id WHERE m.namespace_id=?1 AND m.subject_id=?2 AND (?3 IS NULL OR r.commit_seq<=?3) AND (?4 OR r.revision=(SELECT max(h.revision) FROM memory_revisions h WHERE h.memory_id=m.id AND (?3 IS NULL OR h.commit_seq<=?3))) UNION ALL SELECT o.kind,o.id,r.revision FROM structured_objects o JOIN structured_revisions r ON r.object_id=o.id WHERE o.namespace_id=?1 AND o.subject_id=?2 AND (?3 IS NULL OR r.commit_seq<=?3) AND (?4 OR r.revision=(SELECT max(h.revision) FROM structured_revisions h WHERE h.object_id=o.id AND (?3 IS NULL OR h.commit_seq<=?3))) ORDER BY kind,id,revision")
        .bind(ns.as_str()).bind(subject.as_str()).bind(as_of).bind(history).fetch_all(&mut *conn).await.map_err(storage_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(CognitiveRef {
                kind: crate::codec::parse_enum(row.get("kind"))?,
                id: row.get("id"),
                revision: row.get("revision"),
            })
        })
        .collect()
}
