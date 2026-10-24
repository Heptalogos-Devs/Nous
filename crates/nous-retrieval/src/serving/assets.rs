//! Geometry, independent-root CSR and immutable numerical asset assembly.
use super::*;

pub(super) fn root_rows(prepared: &Prepared) -> Vec<FileTag> {
    let tags = prepared
        .mappings
        .tags
        .iter()
        .map(|(id, key)| (key, *id))
        .collect::<BTreeMap<_, _>>();
    prepared
        .mappings
        .roots
        .iter()
        .flat_map(|(id, root)| {
            prepared.root_tags[root]
                .iter()
                .enumerate()
                .map(|(position, key)| FileTag {
                    file_id: *id,
                    tag_id: tags[key],
                    position: position as i64 + 1,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
pub(super) fn build_assets(prepared: &Prepared) -> Result<(MemoAssets, Value, GraphDisk, Value)> {
    let rows = root_rows(prepared);
    let tags = prepared
        .mappings
        .tags
        .iter()
        .map(|(id, key)| {
            let tag = &prepared.tag_definitions[key];
            GeometryTag {
                id: *id,
                name: tag.label.clone(),
                vector_version: 1,
                vector: prepared.unit_tags[id].to_vec(),
            }
        })
        .collect();
    let geometry = build_geometry(GeometryInput {
        dimension: prepared.identity.dimension,
        model_sig: prepared.identity.space.clone(),
        config: prepared.snapshot.geometry_config.clone(),
        tags,
        memberships: rows.clone(),
        recorded_at_ms: 0,
    })
    .map_err(anyhow::Error::msg)?;
    let tag_vectors = tag_vectors(prepared);
    let artifact = build_artifact(ArtifactInput {
        dimension: prepared.identity.dimension,
        model_sig: prepared.identity.space.clone(),
        generation: prepared.identity.generation.clone(),
        geometry_sig: geometry.signature.clone(),
        rows,
        tag_vectors,
        anchor_gain: geometry.anchor_gain.clone(),
        config: prepared.snapshot.artifact_config.clone(),
    })
    .map_err(anyhow::Error::msg)?;
    let graph = GraphDisk::capture(&artifact.graph);
    let geometry = serde_json::to_value(geometry)?;
    let assets = assemble_assets(prepared, artifact.graph, &geometry, &artifact.metadata)?;
    Ok((assets, geometry, graph, artifact.metadata))
}
pub(super) fn tag_vectors(prepared: &Prepared) -> TagVectorPool {
    prepared.unit_tags.clone()
}
pub(super) fn assemble_assets(
    prepared: &Prepared,
    graph: Arc<NativeArtifact>,
    geometry: &Value,
    artifact: &Value,
) -> Result<MemoAssets> {
    let rows = root_rows(prepared);
    let candidates = prepared
        .mappings
        .candidates
        .iter()
        .map(|(id, mapping)| {
            (
                *id,
                CandidateData {
                    id: *id,
                    file_id: mapping.file_id,
                    source_ids: prepared.mappings.source_sets[&mapping.file_id].clone(),
                    vector: prepared.unit_candidates[id].clone(),
                },
            )
        })
        .collect();
    let files = prepared
        .mappings
        .source_sets
        .iter()
        .map(|(id, roots)| {
            let mut seen = BTreeSet::new();
            let tags = roots
                .iter()
                .flat_map(|root| rows.iter().filter(move |row| row.file_id == *root))
                .filter(|row| seen.insert(row.tag_id))
                .enumerate()
                .map(|(position, row)| FileTag {
                    file_id: *id,
                    tag_id: row.tag_id,
                    position: position as i64 + 1,
                })
                .collect();
            (
                *id,
                FileData {
                    diary_name: "authorized-snapshot".into(),
                    tags,
                },
            )
        })
        .collect();
    let epa = geometry
        .get("epa_cache_json")
        .and_then(Value::as_str)
        .and_then(|raw| decode_epa_basis(raw.to_owned(), prepared.identity.dimension))
        .map(Arc::new);
    MemoAssets::try_new(MemoAssetInput {
        dimension: prepared.identity.dimension,
        signature: artifact["artifactSig"]
            .as_str()
            .context("SERVING_ARTIFACT_INVALID: signature missing")?
            .into(),
        generation: prepared.identity.generation.clone(),
        graph,
        tag_vectors: tag_vectors(prepared),
        tag_names: prepared
            .mappings
            .tags
            .iter()
            .map(|(id, key)| (*id, prepared.tag_definitions[key].label.clone()))
            .collect(),
        epa,
        candidates,
        files,
    })
    .map_err(anyhow::Error::msg)
}
pub(super) fn asset_diagnostics(assets: &MemoAssets, mappings: &Mappings) -> Value {
    let snapshot = assets.snapshot();
    json!({"candidate_count":mappings.candidates.len(),"tag_count":mappings.tags.len(),"source_root_count":mappings.roots.len(),
        "graph_nodes":snapshot.graph.node_ids.len(),"graph_edges":snapshot.graph.targets.len(),"artifact_signature":assets.signature(),"vector_policy":VECTOR_POLICY,
        "numeric_tags":mappings.tags,"source_roots":mappings.roots,"candidate_mapping":mappings.candidates,"vector_coverage":assets.diagnostics()})
}
