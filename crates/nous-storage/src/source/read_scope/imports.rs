use super::*;

/// An explicit whole input permits its faithfully admitted records and exact Import events.
/// This authorization relation never becomes a media dependency or a new explicit grant.
pub(super) async fn allowed(
    conn: &mut SqliteConnection,
    view: &SourceReadView,
    selection: &Selection,
    record: &MaterialId,
    observation: Option<&ObservationId>,
) -> Result<bool> {
    if selection.whole_inputs.is_empty() {
        return Ok(false);
    }
    // UNION deduplicates actual Material identities, including self-links and repeated
    // input chains. LIMIT bounds all discovered ancestors rather than traversal depth.
    let ancestors: Vec<String> = sqlx::query_scalar(
        "WITH RECURSIVE input_chain(material_id) AS (
            SELECT i.material_id
            FROM import_record_containers r
            JOIN import_input_sources i ON i.id=r.input_source_id
                AND i.namespace_id=r.namespace_id AND i.subject_id=r.subject_id
            JOIN observations o ON o.namespace_id=r.namespace_id AND o.subject_id=r.subject_id
                AND o.id=r.observation_id AND o.material_id=r.record_material_id
            WHERE r.namespace_id=?1 AND r.subject_id=?2 AND r.record_material_id=?3
                AND o.kind='import' AND (?4 IS NULL OR r.observation_id=?4)
            UNION
            SELECT i.material_id
            FROM input_chain previous
            JOIN import_record_containers r ON r.record_material_id=previous.material_id
            JOIN import_input_sources i ON i.id=r.input_source_id
                AND i.namespace_id=r.namespace_id AND i.subject_id=r.subject_id
            JOIN observations o ON o.namespace_id=r.namespace_id AND o.subject_id=r.subject_id
                AND o.id=r.observation_id AND o.material_id=r.record_material_id
            WHERE r.namespace_id=?1 AND r.subject_id=?2 AND o.kind='import'
            LIMIT ?5
        ) SELECT material_id FROM input_chain",
    )
    .bind(view.namespace.as_str())
    .bind(view.subject.as_str())
    .bind(record.as_str())
    .bind(observation.map(ObservationId::as_str))
    .bind((MAX_SOURCE_NODES + 1) as i64)
    .fetch_all(conn)
    .await
    .map_err(storage_error)?;
    if ancestors.len() > MAX_SOURCE_NODES {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "import source-read metadata exceeds 1024 nodes",
        ));
    }
    Ok(ancestors
        .into_iter()
        .any(|id| selection.whole_inputs.contains(&MaterialId(id))))
}
