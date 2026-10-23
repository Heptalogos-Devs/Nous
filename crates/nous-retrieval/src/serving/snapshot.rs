//! Canonical candidate sets, local ID mappings and raw input digest.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see UPSTREAM.md.
use super::*;

pub(super) struct Prepared {
    pub(super) identity: VectorGenerationIdentity,
    pub(super) digest: String,
    pub(super) snapshot: ServingSnapshot,
    pub(super) mappings: Mappings,
    pub(super) tag_definitions: BTreeMap<String, ServingTag>,
    pub(super) root_tags: BTreeMap<String, Vec<String>>,
    pub(super) vectors: VectorSnapshot,
    pub(super) unit_candidates: BTreeMap<i64, Arc<[f32]>>,
    pub(super) unit_tags: TagVectorPool,
}
pub(super) fn prepare(snapshot: &ServingSnapshot) -> Result<Prepared> {
    let mut snapshot = snapshot.clone();
    snapshot.candidates.sort_by(|a, b| a.id.cmp(&b.id));
    ensure!(
        snapshot.identity.dimension > 0
            && snapshot.identity.watermark >= 0
            && [&snapshot.identity.space, &snapshot.identity.producer,]
                .iter()
                .all(|s| !s.trim().is_empty()),
        "SERVING_IDENTITY_INVALID"
    );
    let mut tag_definitions = BTreeMap::<String, ServingTag>::new();
    let mut root_tags = BTreeMap::<String, Vec<String>>::new();
    let mut seen = BTreeSet::new();
    for candidate in &mut snapshot.candidates {
        ensure!(
            !candidate.id.trim().is_empty()
                && seen.insert(candidate.id.clone())
                && candidate.revision > 0
                && candidate.commit_seq >= 0
                && candidate.commit_seq <= snapshot.identity.watermark
                && !candidate.source_refs.is_empty(),
            "SERVING_CANDIDATE_INVALID: identity, revision, source root or watermark"
        );
        validate_vector(&candidate.vector, snapshot.identity.dimension)?;
        candidate.source_refs.sort();
        candidate.source_refs.dedup();
        ensure!(
            candidate
                .source_refs
                .iter()
                .all(|root| !root.trim().is_empty()),
            "SERVING_SOURCE_SET_INVALID"
        );
        for tag in &candidate.tags {
            ensure!(
                !tag.key.trim().is_empty()
                    && !tag.label.trim().is_empty()
                    && candidate.source_refs.contains(&tag.source_ref),
                "SERVING_TAG_INVALID: Tag key/label or source root"
            );
            validate_vector(&tag.vector, snapshot.identity.dimension)?;
            if let Some(old) = tag_definitions.get(&tag.key) {
                ensure!(
                    old.label == tag.label && old.vector == tag.vector,
                    "SERVING_TAG_CONFLICT: one key has different label/vector"
                );
            } else {
                tag_definitions.insert(tag.key.clone(), tag.clone());
            }
        }
        let mut membership_seen = BTreeSet::new();
        candidate
            .tags
            .retain(|tag| membership_seen.insert((tag.source_ref.clone(), tag.key.clone())));
        for root in &candidate.source_refs {
            let ordered = candidate
                .tags
                .iter()
                .filter(|tag| &tag.source_ref == root)
                .map(|tag| tag.key.clone())
                .collect::<Vec<_>>();
            if let Some(old) = root_tags.get(root) {
                ensure!(
                    old == &ordered,
                    "SERVING_ROOT_MEMBERSHIP_CONFLICT: one root has different ordered Tags"
                );
            } else {
                root_tags.insert(root.clone(), ordered);
            }
        }
    }
    let digest = hash(&serde_json::to_vec(
        &json!({"vector_policy":VECTOR_POLICY,"space":snapshot.identity.space,"producer":snapshot.identity.producer,"watermark":snapshot.identity.watermark,"dimension":snapshot.identity.dimension,"candidates":snapshot.candidates,"geometry_config":snapshot.geometry_config,"artifact_config":snapshot.artifact_config}),
    )?);
    let mut identity = snapshot.identity.clone();
    identity.generation = format!("g-{}", &digest[..24]);
    identity.signature = digest.clone();
    let roots = root_tags
        .keys()
        .enumerate()
        .map(|(i, root)| Ok((i64::try_from(i + 1)?, root.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let root_ids = roots
        .iter()
        .map(|(id, root)| (root.clone(), *id))
        .collect::<BTreeMap<_, _>>();
    let tags = tag_definitions
        .keys()
        .enumerate()
        .map(|(i, key)| Ok((i64::try_from(i + 1)?, key.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut source_sets = roots
        .keys()
        .map(|id| (*id, vec![*id]))
        .collect::<BTreeMap<_, _>>();
    let mut set_ids = source_sets
        .iter()
        .map(|(id, roots)| (roots.clone(), *id))
        .collect::<BTreeMap<_, _>>();
    let candidates = snapshot
        .candidates
        .iter()
        .enumerate()
        .map(|(i, candidate)| {
            let set = candidate
                .source_refs
                .iter()
                .map(|root| root_ids[root])
                .collect::<Vec<_>>();
            let file_id = if let Some(id) = set_ids.get(&set) {
                *id
            } else {
                let id = i64::try_from(source_sets.len() + 1)?;
                source_sets.insert(id, set.clone());
                set_ids.insert(set, id);
                id
            };
            Ok((
                i64::try_from(i + 1)?,
                CandidateMapping {
                    id: candidate.id.clone(),
                    revision: candidate.revision,
                    commit_seq: candidate.commit_seq,
                    file_id,
                    source_refs: candidate.source_refs.clone(),
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let vectors = VectorSnapshot {
        identity: identity.clone(),
        candidates: snapshot
            .candidates
            .iter()
            .zip(candidates.keys())
            .map(|(candidate, id)| VectorRecord {
                id: *id,
                source_key: format!("candidate:{}@{}", candidate.id, candidate.revision),
                vector: candidate.vector.clone(),
            })
            .collect(),
        tags: tags
            .iter()
            .map(|(id, key)| VectorRecord {
                id: *id,
                source_key: key.clone(),
                vector: tag_definitions[key].vector.clone(),
            })
            .collect(),
    };
    // Keep raw ANN inputs for exact original-vector manifests. Both the adapter
    // and these immutable numerical assets derive unit F32 from those raw bytes,
    // using the same f64 accumulation/division. Never feed derived vectors back
    // into snapshot normalization or persistence identity.
    let unit_candidates = vectors
        .candidates
        .iter()
        .map(|record| {
            Ok((
                record.id,
                Arc::from(crate::vector::unit_vector(
                    &record.vector,
                    identity.dimension,
                )?),
            ))
        })
        .collect::<Result<_>>()?;
    let unit_tags = vectors
        .tags
        .iter()
        .map(|record| {
            Ok((
                record.id,
                Arc::from(crate::vector::unit_vector(
                    &record.vector,
                    identity.dimension,
                )?),
            ))
        })
        .collect::<Result<_>>()?;
    Ok(Prepared {
        identity,
        digest,
        snapshot,
        mappings: Mappings {
            candidates,
            tags,
            roots,
            source_sets,
        },
        tag_definitions,
        root_tags,
        vectors,
        unit_candidates,
        unit_tags,
    })
}
pub(super) fn validate_vector(vector: &[f32], dimension: usize) -> Result<()> {
    ensure!(
        vector.len() == dimension
            && vector.iter().all(|v| v.is_finite())
            && vector.iter().any(|v| *v != 0.0),
        "SERVING_VECTOR_INVALID: dimension/finite/nonzero"
    );
    Ok(())
}
