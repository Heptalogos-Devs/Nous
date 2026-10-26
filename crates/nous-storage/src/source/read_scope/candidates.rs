use super::*;

/// Complete metadata authorization precedes snapshot limits and retrieval ranking.
pub(crate) async fn cognitive_keys(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    as_of: Option<i64>,
    scope: &SourceReadView,
) -> Result<String> {
    if scope.constraints.is_empty() {
        return Ok("null".into());
    }
    let rows = sqlx::query("SELECT 'memory' AS kind,m.id,max(r.revision) AS revision FROM memories m JOIN memory_revisions r ON r.memory_id=m.id WHERE m.namespace_id=?1 AND m.subject_id=?2 AND (?3 IS NULL OR r.commit_seq<=?3) GROUP BY m.id UNION ALL SELECT o.kind,o.id,max(r.revision) FROM structured_objects o JOIN structured_revisions r ON r.object_id=o.id WHERE o.namespace_id=?1 AND o.subject_id=?2 AND (?3 IS NULL OR r.commit_seq<=?3) GROUP BY o.id ORDER BY kind,id")
        .bind(ns.as_str()).bind(subject.as_str()).bind(as_of).fetch_all(&mut *conn).await.map_err(storage_error)?;
    let mut permitted = Vec::new();
    for row in rows {
        let reference = CognitiveRef {
            kind: crate::codec::parse_enum(row.get("kind"))?,
            id: row.get("id"),
            revision: row.get("revision"),
        };
        if target_allowed(conn, scope, &RetrievalTarget::Cognition(reference.clone())).await? {
            permitted.push(reference.to_string());
        }
    }
    Ok(serde_json::to_string(&permitted)?)
}
