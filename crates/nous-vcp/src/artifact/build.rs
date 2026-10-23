//! Artifact assembly and content signatures.
// Adapted from VCPToolBox d1b40c863273cbbba082fb7657671424768c4a53.
// CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
use super::*;

/// Build the same fact/transport/provenance arithmetic from a frozen input snapshot.
pub fn build_artifact(mut input: ArtifactInput) -> Result<ArtifactOutput, String> {
    if input.dimension == 0 || input.model_sig.trim().is_empty() || input.generation.is_empty() {
        return Err("artifact requires dimension, model signature and generation".into());
    }
    if input
        .rows
        .iter()
        .any(|row| row.file_id <= 0 || row.tag_id <= 0)
    {
        return Err("artifact memberships require positive identifiers".into());
    }
    if input
        .anchor_gain
        .iter()
        .any(|(id, gain)| *id <= 0 || !gain.is_finite() || *gain < 0.0)
    {
        return Err(
            "MEMO_ANCHOR_GAIN_INVALID: positive IDs and finite nonnegative gains required".into(),
        );
    }
    input
        .rows
        .sort_by_key(|row| (row.file_id, row.position, row.tag_id));
    let mut vectors = HashMap::new();
    for (id, vector) in &input.tag_vectors {
        if *id <= 0
            || vector.len() != input.dimension
            || vector.iter().any(|value| !value.is_finite())
        {
            return Err(format!("invalid artifact vector {id}"));
        }
        let mut unit: Vec<f64> = vector.iter().map(|value| f64::from(*value)).collect();
        let norm = unit
            .iter()
            .fold(0.0, |sum, value| sum + value * value)
            .sqrt();
        if norm > 1e-12 {
            for value in &mut unit {
                *value /= norm;
            }
            vectors.insert(*id, unit);
        }
    }
    let fact = build_fact_matrix(&input.rows, &vectors, &input.anchor_gain, &input.config)?;
    let (transport, wormholes) = build_transport(&fact, &input.anchor_gain, &input.config);
    let provenance = build_provenance(&input.rows, &input.config);

    let mut inbound = HashMap::new();
    let mut sources: Vec<_> = transport.keys().copied().collect();
    sources.sort_unstable();
    let mut graph_hasher = Sha256::new();
    graph_hasher.update(b"memo-graph-le-v2");
    graph_hasher.update((sources.len() as u64).to_le_bytes());
    for source in sources {
        let mut edges: Vec<_> = transport[&source].iter().collect();
        edges.sort_by_key(|(target, _)| **target);
        graph_hasher.update(source.to_le_bytes());
        graph_hasher.update((edges.len() as u64).to_le_bytes());
        for (target, weight) in edges {
            *inbound.entry(*target).or_default() += *weight;
            graph_hasher.update(target.to_le_bytes());
            graph_hasher.update(weight.to_le_bytes());
        }
    }
    let graph_digest = format!("{:x}", graph_hasher.finalize())[..48].to_owned();
    let mut provenance_hasher = Sha256::new();
    provenance_hasher.update(b"memo-provenance-le-v2");
    provenance_hasher.update((provenance.len() as u64).to_le_bytes());
    for ((source, target), values) in &provenance {
        provenance_hasher.update(source.to_le_bytes());
        provenance_hasher.update(target.to_le_bytes());
        provenance_hasher.update((values.len() as u64).to_le_bytes());
        for value in values {
            provenance_hasher.update(value.file_id.to_le_bytes());
            provenance_hasher.update(value.mass.to_le_bytes());
        }
    }
    let provenance_generation = format!("{:x}", provenance_hasher.finalize())[..40].to_owned();
    let config_json = serde_json::to_string(&input.config)
        .map_err(|error| format!("encode effective config failed: {}", error))?;
    let config_hash = hash_text(&config_json, 32);
    let source_artifact_sig = hash_text(
        &format!(
            "{}|{}|{}|{}",
            V9_ALGORITHM_VERSION, input.model_sig, graph_digest, input.geometry_sig
        ),
        24,
    );
    let artifact_sig = hash_text(
        &format!(
            "{}|{}|{}|{}|{}|{}",
            ALGORITHM_VERSION,
            source_artifact_sig,
            graph_digest,
            provenance_generation,
            input.generation,
            config_hash
        ),
        48,
    );

    let native = build_native_artifact(
        &transport,
        inbound,
        input.anchor_gain,
        wormholes,
        &provenance,
    );
    drop(transport);

    let metadata = json!({"algorithmVersion":ALGORITHM_VERSION,"artifactSig":artifact_sig,
        "sourceArtifactSig":source_artifact_sig,"graphGeneration":graph_digest,
        "provenanceGeneration":provenance_generation,"generation":input.generation,
        "modelSig":input.model_sig,"configHash":config_hash,"maxInbound":native.max_inbound,
        "effectiveConfig":input.config});
    Ok(ArtifactOutput {
        graph: native,
        metadata,
    })
}
