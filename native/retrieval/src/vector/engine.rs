//! Native HNSW construction, readback and cosine normalization.
use super::*;

pub(super) fn index_options(dimension: usize) -> IndexOptions {
    // Source DenseGeneration settings, with unique F32 cosine vectors.
    IndexOptions {
        dimensions: dimension,
        metric: MetricKind::Cos,
        quantization: ScalarKind::F32,
        connectivity: 16,
        expansion_add: 40,
        expansion_search: 32,
        multi: false,
    }
}

pub(super) fn build_index(dimension: usize, records: &[VectorRecord]) -> Result<Index> {
    let index = Index::new(&index_options(dimension)).context("USearch initialization")?;
    index
        .reserve(records.len().max(1))
        .context("USearch reserve")?;
    // Stable insertion order for the same frozen source snapshot.
    let ordered: BTreeMap<_, _> = records.iter().map(|record| (record.id, record)).collect();
    for (id, record) in ordered {
        index
            .add(id as u64, &unit_vector(&record.vector, dimension)?)
            .context("USearch add")?;
    }
    Ok(index)
}

pub(super) fn load_index(
    path: &Path,
    identity: &VectorGenerationIdentity,
    records: &BTreeMap<i64, RecordIdentity>,
) -> Result<Index> {
    let header = Index::metadata(utf8_path(path)?).context("USearch header")?;
    ensure!(
        header.dimensions == identity.dimension as u64
            && header.metric == MetricKind::Cos
            && header.quantization == ScalarKind::F32
            && !header.multi
            && header.count_present == records.len() as u64
            && header.count_deleted == 0,
        "VECTOR_INDEX_FORMAT_MISMATCH: dimension, cosine/F32/unique format or record count changed"
    );
    let index = Index::new(&index_options(identity.dimension)).context("USearch initialization")?;
    index.load(utf8_path(path)?).context("USearch load")?;
    ensure!(
        index.size() == records.len()
            && index.dimensions() == identity.dimension
            && index.metric_kind() == MetricKind::Cos
            && index.scalar_kind() == ScalarKind::F32
            && !index.multi(),
        "VECTOR_INDEX_FORMAT_MISMATCH: loaded index disagrees"
    );
    let mut vector = vec![0.0; identity.dimension];
    for (id, record) in records {
        let count = index
            .get(*id as u64, &mut vector)
            .context("USearch vector readback")?;
        ensure!(
            count == 1
                && vector.iter().all(|value: &f32| value.is_finite())
                && vector_hash(&vector) == record.index_vector_sha256,
            "VECTOR_INDEX_CONTENT_MISMATCH: missing ID or changed stored vector"
        );
    }
    Ok(index)
}

pub(crate) fn unit_vector(vector: &[f32], dimension: usize) -> Result<Vec<f32>> {
    ensure!(
        vector.len() == dimension && vector.iter().all(|value| value.is_finite()),
        "VECTOR_VALUES_INVALID: dimension or non-finite value"
    );
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    ensure!(
        norm > 0.0 && norm.is_finite(),
        "VECTOR_ZERO: cosine vectors must be nonzero"
    );
    // Normalization in f64 keeps native F32 cosine away from overflow/underflow.
    Ok(vector
        .iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect())
}

pub(super) fn vector_hash(vector: &[f32]) -> String {
    let mut hash = Sha256::new();
    for value in vector {
        hash.update(value.to_bits().to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}
pub(super) fn utf8_path(path: &Path) -> Result<&str> {
    path.to_str()
        .context("VECTOR_PATH_INVALID: USearch needs UTF-8 path")
}
