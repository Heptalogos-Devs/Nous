//! Construct the mathematical graph and its reusable in-memory assets.
use super::*;

pub(super) fn build_math(
    input: &Input,
    concepts: &ConceptMeta,
    cancel: &CancellationToken,
) -> Result<Option<MathDisk>> {
    let Some(vectors) = &input.concepts.vectors else {
        return Ok(None);
    };
    if input.candidates.vectors.is_none() {
        return Ok(None);
    }
    let mut rows = Vec::new();
    for seq in 0..input.facts.sequence_roots.len() {
        for cursor in input.facts.indptr[seq] as usize..input.facts.indptr[seq + 1] as usize {
            rows.push(FileTag {
                file_id: seq as i64 + 1,
                tag_id: input.facts.indices[cursor] as i64 + 1,
                position: if input.facts.ordered[seq] {
                    input.facts.positions[cursor]
                } else {
                    0
                },
            });
        }
    }
    let pool = concepts
        .keys
        .iter()
        .enumerate()
        .map(|(row, _)| Ok((row as i64 + 1, Arc::<[f32]>::from(unit(vectors.row(row))?))))
        .collect::<Result<HashMap<_, _>>>()?;
    let assoc = &input.associations;
    let meta: AssociationMeta = serde_json::from_str(&assoc.metadata_json)?;
    let association_options = &input.options["associations"];
    let enabled = association_options
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let relation_weights = association_options
        .get("relation_gains")
        .and_then(Value::as_object);
    let facts: FactMeta = serde_json::from_str(&input.facts.metadata_json)?;
    let mut canonical_groups = BTreeMap::<(String, Vec<u64>), usize>::new();
    let mut canonical_ids = Vec::new();
    for (i, g) in meta.support_groups.iter().enumerate() {
        let mut roots = slice(&assoc.group_root_indptr, &assoc.group_root_indices, i).to_vec();
        roots.sort_unstable();
        roots.dedup();
        canonical_ids.push(*canonical_groups.entry((g.kind.clone(), roots)).or_insert(i));
    }
    let mut masses = BTreeMap::<(i64, i64, String, i64), f64>::new();
    if enabled {
        for i in 0..assoc.sources.len() {
            if assoc.polarities[i] < 0 {
                continue;
            }
            let relation = &meta.relations[i];
            let gain = relation_weights
                .and_then(|w| w.get(relation))
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    if relation_weights.is_none()
                        && matches!(relation.as_str(), "related" | "supports" | "sequence")
                    {
                        1.0
                    } else {
                        0.0
                    }
                });
            ensure!(
                gain.is_finite() && gain >= 0.0,
                "Association projection gain invalid"
            );
            let group = &meta.support_groups[assoc.edge_groups[i] as usize];
            let interpretation_gain = association_options
                .get("interpretation_gain")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            ensure!(
                interpretation_gain.is_finite() && interpretation_gain >= 0.0,
                "Invalid interpretation_gain"
            );
            let kind_gain = match group.kind.as_str() {
                "source" => 1.0,
                "interpretation" => interpretation_gain,
                _ => 0.0,
            };
            let weight = gain * assoc.strengths[i] * assoc.confidences[i] * kind_gain;
            let canonical = canonical_ids[assoc.edge_groups[i] as usize];
            let group_id = facts.root_keys.len() as i64 + 1 + canonical as i64;
            for (source, target) in [
                (assoc.sources[i] as i64 + 1, assoc.targets[i] as i64 + 1),
                (assoc.targets[i] as i64 + 1, assoc.sources[i] as i64 + 1),
            ]
            .into_iter()
            .take(if assoc.directed[i] { 1 } else { 2 })
            {
                let mass = masses
                    .entry((source, target, relation.clone(), group_id))
                    .or_default();
                *mass = mass.max(weight);
            }
        }
    }
    let additional_facts = masses
        .into_iter()
        .filter(|(_, mass)| *mass > 0.0)
        .map(
            |((source, target, _, root), mass)| nous_vcp::artifact::FactEdge {
                source,
                target,
                root,
                mass,
            },
        )
        .collect::<Vec<_>>();
    let edges = additional_facts
        .iter()
        .map(|e| (e.source, e.target, e.mass))
        .collect::<Vec<_>>();
    let native_options = input.options.get("native").unwrap_or(&input.options);
    let build_config: nous_vcp::config::BuildConfig =
        serde_json::from_value(native_options.clone())
            .map_err(|e| failure("wave.invalid_input", e))?;
    build_config
        .validate()
        .map_err(|e| failure("wave.invalid_input", e))?;
    cancel.check()?;
    let geometry = build_geometry(
        GeometryInput {
            dimension: vectors.cols,
            model_sig: input.identity["space_signature"].as_str().unwrap().into(),
            config: serde_json::to_value(&build_config.geometry)?,
            tags: concepts
                .keys
                .iter()
                .enumerate()
                .map(|(row, _)| GeometryTag {
                    id: row as i64 + 1,
                    name: concepts.labels[row].clone(),
                    vector_version: 1,
                    vector: pool[&(row as i64 + 1)].to_vec(),
                })
                .collect(),
            memberships: rows.clone(),
            edges: if edges.is_empty() {
                None
            } else {
                Some(edges.clone())
            },
            recorded_at_ms: 0,
        },
        Some(&|| cancel.is_cancelled()),
    )
    .map_err(|e| {
        failure(
            if cancel.is_cancelled() {
                "wave.cancelled"
            } else {
                "wave.geometry_failed"
            },
            e,
        )
    })?;
    cancel.check()?;
    let config = serde_json::to_value(&build_config.artifact)?;
    let built = build_artifact(
        ArtifactInput {
            dimension: vectors.cols,
            model_sig: input.identity["space_signature"].as_str().unwrap().into(),
            generation: input.identity["snapshot_id"].as_str().unwrap().into(),
            geometry_sig: geometry.signature.clone(),
            rows,
            tag_vectors: pool,
            anchor_gain: geometry.anchor_gain.clone(),
            config: config.clone(),
            source_roots: input
                .facts
                .sequence_roots
                .iter()
                .enumerate()
                .map(|(seq, root)| (seq as i64 + 1, *root as i64 + 1))
                .collect(),
            additional_facts,
        },
        Some(&|| cancel.is_cancelled()),
    )
    .map_err(|e| {
        failure(
            if cancel.is_cancelled() {
                "wave.cancelled"
            } else {
                "wave.artifact_failed"
            },
            e,
        )
    })?;
    let mut graph = (*built.graph).clone();
    let facts: FactMeta = serde_json::from_str(&input.facts.metadata_json)?;
    for (i, g) in meta.support_groups.iter().enumerate() {
        if canonical_ids[i] != i {
            continue;
        }
        graph.support_groups.insert(
            facts.root_keys.len() as i64 + 1 + i as i64,
            nous_vcp::graph::SupportGroup {
                required_roots: slice(&assoc.group_root_indptr, &assoc.group_root_indices, i)
                    .iter()
                    .map(|r| *r as i64 + 1)
                    .collect(),
                kind: g.kind.clone(),
            },
        );
    }
    let signature = format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(rmp_serde::to_vec_named(&graph)?)
    );
    let epa_json = geometry.epa_cache_json.clone();
    Ok(Some(MathDisk {
        graph: Arc::new(graph),
        epa_json,
        signature,
        geometry: json!({"algorithm_version":geometry.algorithm_version,"signature":geometry.signature,"tag_count":geometry.tag_count,"computed_count":geometry.computed_count}),
    }))
}
pub(super) fn assemble(
    input: &Input,
    c: &CandidateMeta,
    t: &ConceptMeta,
    f: &FactMeta,
    math: &MathDisk,
) -> Result<MemoAssets> {
    let dimension = input.concepts.vectors.as_ref().unwrap().cols;
    let mut files = BTreeMap::new();
    let mut roots = BTreeMap::<i64, RootFact>::new();
    for (row, checksum) in f.root_checksums.iter().enumerate() {
        roots.insert(
            row as i64 + 1,
            RootFact {
                required_roots: vec![row as i64 + 1],
                checksum: checksum.clone(),
                tags: BTreeSet::new(),
            },
        );
    }
    for seq in 0..f.sequence_keys.len() {
        let root = input.facts.sequence_roots[seq] as i64 + 1;
        for tag in slice(&input.facts.indptr, &input.facts.indices, seq) {
            roots.get_mut(&root).unwrap().tags.insert(*tag as i64 + 1);
        }
    }
    for (root, fact) in &roots {
        files.insert(
            *root,
            FileData {
                diary_name: "source".into(),
                tags: fact
                    .tags
                    .iter()
                    .map(|tag| FileTag {
                        file_id: *root,
                        tag_id: *tag,
                        position: 0,
                    })
                    .collect(),
            },
        );
    }
    // Explicit source groups retain their complete premises for strong-route witnesses.
    // Interpretation groups never create an original-source witness.
    let mut root_groups = roots
        .iter()
        .map(|(id, fact)| (fact.required_roots.clone(), *id))
        .collect::<BTreeMap<_, _>>();
    let assoc = &input.associations;
    let metadata: AssociationMeta = serde_json::from_str(&assoc.metadata_json)?;
    for i in 0..assoc.sources.len() {
        let group_index = assoc.edge_groups[i] as usize;
        let group = &metadata.support_groups[group_index];
        if assoc.polarities[i] < 0 || group.kind != "source" {
            continue;
        }
        let mut required = slice(
            &assoc.group_root_indptr,
            &assoc.group_root_indices,
            group_index,
        )
        .iter()
        .map(|r| *r as i64 + 1)
        .collect::<Vec<_>>();
        required.sort_unstable();
        required.dedup();
        if required.is_empty() {
            continue;
        }
        let id = *root_groups
            .entry(required.clone())
            .or_insert(f.root_keys.len() as i64 + 1 + group_index as i64);
        let checksum = required
            .iter()
            .map(|r| f.root_checksums[(*r - 1) as usize].as_str())
            .collect::<Vec<_>>()
            .join("+");
        let fact = roots.entry(id).or_insert_with(|| RootFact {
            required_roots: required,
            checksum,
            tags: BTreeSet::new(),
        });
        fact.tags.insert(assoc.sources[i] as i64 + 1);
        fact.tags.insert(assoc.targets[i] as i64 + 1);
    }
    let docs = c
        .documents
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(i, key)| (key, i as i64 + 1))
        .collect::<BTreeMap<_, _>>();
    let mut candidates = BTreeMap::new();
    let mut candidate_paths = BTreeMap::new();
    let candidate_vectors = input
        .candidates
        .vectors
        .as_ref()
        .context("VCP requires candidate vectors")?;
    for row in 0..c.keys.len() {
        let file_id = f.root_keys.len() as i64 + row as i64 + 1;
        let mut tags = Vec::new();
        let mut ordered = BTreeSet::new();
        let mut paths = Vec::new();
        for path in input.candidates.path_indptr[row] as usize
            ..input.candidates.path_indptr[row + 1] as usize
        {
            let mut steps = Vec::new();
            for cursor in input.candidates.path_step_indptr[path] as usize
                ..input.candidates.path_step_indptr[path + 1] as usize
            {
                steps.push(FileTag {
                    file_id,
                    tag_id: input.candidates.path_indices[cursor] as i64 + 1,
                    position: input.candidates.path_positions[cursor],
                });
            }
            paths.push(steps);
        }
        if let Some(first) = paths.first() {
            for tag in first {
                ordered.insert(tag.tag_id);
                tags.push(tag.clone());
            }
        }
        candidate_paths.insert(row as i64 + 1, paths);
        for tag in slice(
            &input.candidates.membership_indptr,
            &input.candidates.membership_indices,
            row,
        ) {
            let tag = *tag as i64 + 1;
            if !ordered.contains(&tag) {
                tags.push(FileTag {
                    file_id,
                    tag_id: tag,
                    position: 0,
                });
            }
        }
        files.insert(
            file_id,
            FileData {
                diary_name: c.documents[row].clone(),
                tags,
            },
        );
        candidates.insert(
            row as i64 + 1,
            CandidateData {
                id: row as i64 + 1,
                file_id,
                document_id: docs[&c.documents[row]],
                source_ids: slice(
                    &input.candidates.root_indptr,
                    &input.candidates.root_indices,
                    row,
                )
                .iter()
                .map(|r| *r as i64 + 1)
                .collect(),
                vector: Arc::from(unit(candidate_vectors.row(row))?),
            },
        );
    }
    MemoAssets::try_new(MemoAssetInput {
        tag_aliases: t
            .aliases
            .iter()
            .enumerate()
            .map(|(i, aliases)| (i as i64 + 1, aliases.clone()))
            .collect(),
        root_facts: roots,
        dimension,
        signature: math.signature.clone(),
        generation: input.identity["snapshot_id"].as_str().unwrap().into(),
        graph: math.graph.clone(),
        tag_vectors: t
            .keys
            .iter()
            .enumerate()
            .map(|(row, _)| {
                Ok((
                    row as i64 + 1,
                    Arc::from(unit(input.concepts.vectors.as_ref().unwrap().row(row))?),
                ))
            })
            .collect::<Result<_>>()?,
        tag_names: t
            .labels
            .iter()
            .enumerate()
            .map(|(i, name)| (i as i64 + 1, name.clone()))
            .collect(),
        epa: nous_vcp::pipeline::decode_epa_basis(math.epa_json.clone(), dimension).map(Arc::new),
        candidates,
        candidate_paths,
        files,
    })
    .map_err(|e| failure("wave.assets_invalid", e))
}
