//! Frozen lane mappings and source vector integrity.
use super::*;

pub(super) fn snapshot_metadata(snapshot: &VectorSnapshot) -> Result<FrozenMetadata> {
    let identity = &snapshot.identity;
    ensure!(
        identity.dimension > 0
            && identity.watermark >= 0
            && [&identity.space, &identity.generation, &identity.signature]
                .iter()
                .all(|value| !value.trim().is_empty()),
        "VECTOR_IDENTITY_INVALID: dimension, nonnegative watermark and all signatures required"
    );
    let candidates = record_metadata(&snapshot.candidates, identity.dimension)?;
    let tags = record_metadata(&snapshot.tags, identity.dimension)?;
    Ok(FrozenMetadata {
        identity: identity.clone(),
        candidates,
        tags,
    })
}

pub(super) fn record_metadata(
    records: &[VectorRecord],
    dimension: usize,
) -> Result<BTreeMap<i64, RecordIdentity>> {
    let mut output = BTreeMap::new();
    let mut source_keys = BTreeSet::new();
    for record in records {
        ensure!(
            record.id > 0 && !record.source_key.trim().is_empty(),
            "VECTOR_RECORD_INVALID: positive ID and source key required"
        );
        ensure!(
            source_keys.insert(&record.source_key),
            "VECTOR_SOURCE_MAPPING_CONFLICT: duplicate source key within one lane"
        );
        let unit = unit_vector(&record.vector, dimension)?;
        let identity = RecordIdentity {
            source_key: record.source_key.clone(),
            source_vector_sha256: vector_hash(&record.vector),
            index_vector_sha256: vector_hash(&unit),
        };
        ensure!(
            output.insert(record.id, identity).is_none(),
            "VECTOR_ID_DUPLICATE: duplicate ID in one lane"
        );
    }
    Ok(output)
}
