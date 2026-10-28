use super::*;

/// Read grants flow through actual adopted unit outputs, never from model metadata.
pub(super) async fn allowed(
    conn: &mut SqliteConnection,
    view: &SourceReadView,
    selection: &Selection,
    event: &ObservationId,
    source: Option<&MediaReference>,
    memories: Option<&BTreeSet<CognitiveRef>>,
) -> Result<bool> {
    if selection.whole_inputs.is_empty() && selection.explicit_regions.is_empty() {
        return Ok(false);
    }
    if let Some(memories) = memories {
        for memory in memories {
            let rows = output_sources(conn, view, event, Some(memory)).await?;
            if !permitted(selection, source, &rows) {
                return Ok(false);
            }
        }
        return Ok(!memories.is_empty());
    }
    let rows = output_sources(conn, view, event, None).await?;
    Ok(permitted(selection, source, &rows))
}

async fn output_sources(
    conn: &mut SqliteConnection,
    view: &SourceReadView,
    event: &ObservationId,
    memory: Option<&CognitiveRef>,
) -> Result<Vec<(String, String, String)>> {
    let rows:Vec<(String,String,String)>=sqlx::query_as(
        "SELECT DISTINCT m.memory_id,u.source_region_id,i.material_id
        FROM import_unit_outputs result
        JOIN json_each(result.body,'$.memory_ids') declared
        JOIN memory_revisions m ON m.memory_id=declared.value
            AND m.namespace_id=result.namespace_id AND m.subject_id=result.subject_id
        JOIN import_text_units u ON u.id=result.unit_id
            AND u.namespace_id=m.namespace_id AND u.subject_id=m.subject_id
        JOIN import_input_sources i ON i.id=u.input_source_id
            AND i.namespace_id=u.namespace_id AND i.subject_id=u.subject_id
        JOIN source_regions region ON region.id=u.source_region_id
            AND region.namespace_id=u.namespace_id AND region.material_id=i.material_id
        JOIN observations actual ON actual.namespace_id=m.namespace_id AND actual.subject_id=m.subject_id
            AND actual.id=m.observation_id AND actual.material_id=m.material_id
        WHERE result.namespace_id=?1 AND result.subject_id=?2 AND result.observation_id=actual.id AND actual.id=?3 AND actual.kind='import'
            AND m.material_id=i.material_id AND json_extract(m.source,'$.kind')='source_region'
            AND json_extract(m.source,'$.id')=u.source_region_id
            AND (?4 IS NULL OR (m.memory_id=?4 AND m.revision=?5))
        LIMIT ?6",
    ).bind(view.namespace.as_str()).bind(view.subject.as_str()).bind(event.as_str())
        .bind(memory.map(|value|value.id.as_str())).bind(memory.map(|value|value.revision))
        .bind((MAX_SOURCE_NODES+1)as i64).fetch_all(conn).await.map_err(storage_error)?;
    if rows.len() > MAX_SOURCE_NODES {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "unit output source-read metadata exceeds 1024 nodes",
        ));
    }
    Ok(rows)
}
fn permitted(
    selection: &Selection,
    source: Option<&MediaReference>,
    rows: &[(String, String, String)],
) -> bool {
    rows.iter().any(|(_, region, material)| {
        let region = media::SourceRegionId(region.clone());
        let material = MaterialId(material.clone());
        let whole = selection.whole_inputs.contains(&material);
        let exact_region = selection.explicit_regions.contains(&region);
        let locator = match source {
            None => true,
            Some(MediaReference::SourceRegion { id }) => id == &region,
            Some(MediaReference::Original { material_id }) => whole && material_id == &material,
            _ => false,
        };
        locator && (whole || exact_region)
    })
}
