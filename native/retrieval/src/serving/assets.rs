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
    let explicit_edges = explicit_edges(prepared)?;
    let geometry = build_geometry(GeometryInput {
        dimension: prepared.identity.dimension,
        model_sig: prepared.identity.space.clone(),
        config: prepared.snapshot.geometry_config.clone(),
        tags,
        memberships: rows.clone(),
        edges: explicit_edges.clone(),
        recorded_at_ms: 0,
    })
    .map_err(anyhow::Error::msg)?;
    let tag_vectors = tag_vectors(prepared);
    let artifact = if let Some(edges) = &explicit_edges {
        let graph = nous_vcp::artifact::graph_from_edges(
            edges,
            geometry.anchor_gain.clone(),
            &prepared.snapshot.artifact_config,
        )
        .map_err(anyhow::Error::msg)?;
        let mut owned = (*graph).clone();
        if let Some(explicit) = &prepared.snapshot.graph {
            let tag_ids = prepared
                .mappings
                .tags
                .iter()
                .map(|(id, key)| (key, *id))
                .collect::<BTreeMap<_, _>>();
            let root_ids = prepared
                .mappings
                .roots
                .iter()
                .map(|(id, key)| (key, *id))
                .collect::<BTreeMap<_, _>>();
            for row in 0..explicit.tag_keys.len() {
                let Some(&source) = tag_ids.get(&explicit.tag_keys[row]) else {
                    continue;
                };
                for cursor in explicit.indptr[row] as usize..explicit.indptr[row + 1] as usize {
                    let Some(&target) =
                        tag_ids.get(&explicit.tag_keys[explicit.indices[cursor] as usize])
                    else {
                        continue;
                    };
                    if explicit.polarities[cursor] < 0 {
                        continue;
                    }
                    let roots = explicit.source_indices[explicit.source_indptr[cursor] as usize
                        ..explicit.source_indptr[cursor + 1] as usize]
                        .iter()
                        .filter_map(|index| {
                            root_ids
                                .get(&explicit.source_references[*index as usize])
                                .copied()
                        })
                        .collect::<BTreeSet<_>>();
                    let count = roots.len().max(1) as f64;
                    let contributions = owned.provenance.entry((source, target)).or_default();
                    for root in roots {
                        contributions.push((root, explicit.weights[cursor] / count));
                    }
                }
            }
        }
        let graph = Arc::new(owned.try_prepared().map_err(anyhow::Error::msg)?);
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(
                &json!({"identity":prepared.identity,"graph":prepared.snapshot.graph,"geometry":geometry.signature})
            )?)
        );
        nous_vcp::artifact::ArtifactOutput {
            graph,
            metadata: json!({"artifactSig":digest,"graph_source":"explicit","positive_edges":edges.len(),"negative_edges":prepared.snapshot.graph.as_ref().map(|graph|graph.polarities.iter().filter(|polarity|**polarity<0).count()).unwrap_or(0)}),
        }
    } else {
        build_artifact(ArtifactInput {
            dimension: prepared.identity.dimension,
            model_sig: prepared.identity.space.clone(),
            generation: prepared.identity.generation.clone(),
            geometry_sig: geometry.signature.clone(),
            rows,
            tag_vectors,
            anchor_gain: geometry.anchor_gain.clone(),
            config: prepared.snapshot.artifact_config.clone(),
        })
        .map_err(anyhow::Error::msg)?
    };
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
            let candidate = prepared
                .mappings
                .candidates
                .values()
                .find(|candidate| candidate.file_id == *id);
            let tag_ids = prepared
                .mappings
                .tags
                .iter()
                .map(|(id, key)| (key, *id))
                .collect::<BTreeMap<_, _>>();
            let membership: Vec<i64> = if let Some(candidate) = candidate {
                candidate
                    .tag_keys
                    .iter()
                    .filter_map(|key| tag_ids.get(key).copied())
                    .collect()
            } else {
                roots
                    .iter()
                    .flat_map(|root| rows.iter().filter(move |row| row.file_id == *root))
                    .filter(|row| seen.insert(row.tag_id))
                    .map(|row| row.tag_id)
                    .collect()
            };
            let tags = membership
                .into_iter()
                .enumerate()
                .map(|(position, tag_id)| FileTag {
                    file_id: *id,
                    tag_id,
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

fn explicit_edges(prepared: &Prepared) -> Result<Option<Vec<(i64, i64, f64)>>> {
    let Some(graph) = &prepared.snapshot.graph else {
        return Ok(None);
    };
    let n = graph.tag_keys.len();
    let m = graph.indices.len();
    ensure!(
        graph.indptr.len() == n + 1
            && graph.indptr.first() == Some(&0)
            && graph.indptr.last() == Some(&(m as u64))
            && graph.indptr.windows(2).all(|pair| pair[0] <= pair[1]),
        "invalid explicit graph offsets"
    );
    ensure!(
        graph.weights.len() == m
            && graph.polarities.len() == m
            && graph.indices.iter().all(|index| (*index as usize) < n)
            && graph
                .weights
                .iter()
                .all(|weight| weight.is_finite() && *weight >= 0.0)
            && graph
                .polarities
                .iter()
                .all(|polarity| [-1, 1].contains(polarity)),
        "invalid explicit graph edge arrays"
    );
    ensure!(
        graph.source_indptr.len() == m + 1
            && graph.source_indptr.first() == Some(&0)
            && graph.source_indptr.last() == Some(&(graph.source_indices.len() as u64))
            && graph
                .source_indptr
                .windows(2)
                .all(|pair| pair[0] <= pair[1])
            && graph
                .source_indices
                .iter()
                .all(|id| (*id as usize) < graph.source_references.len()),
        "invalid graph source CSR"
    );
    let mapping = prepared
        .mappings
        .tags
        .iter()
        .map(|(id, key)| (key, *id))
        .collect::<BTreeMap<_, _>>();
    let mut edges = Vec::new();
    for row in 0..n {
        let Some(&source) = mapping.get(&graph.tag_keys[row]) else {
            continue;
        };
        for cursor in graph.indptr[row] as usize..graph.indptr[row + 1] as usize {
            let Some(&target) = mapping.get(&graph.tag_keys[graph.indices[cursor] as usize]) else {
                continue;
            };
            if graph.polarities[cursor] > 0 {
                edges.push((source, target, graph.weights[cursor]));
            }
        }
    }
    Ok(Some(edges))
}
