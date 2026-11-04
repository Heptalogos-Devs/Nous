use anyhow::{Result, ensure};
use std::sync::Arc;
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Matrix {
    pub values: Arc<Vec<f32>>,
    pub rows: usize,
    pub cols: usize,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Candidates {
    pub metadata_json: String,
    pub texts: Arc<Vec<String>>,
    pub vectors: Option<Matrix>,
    pub membership_indptr: Arc<Vec<u64>>,
    pub membership_indices: Arc<Vec<u64>>,
    pub path_indptr: Arc<Vec<u64>>,
    pub path_step_indptr: Arc<Vec<u64>>,
    pub path_indices: Arc<Vec<u64>>,
    pub path_positions: Arc<Vec<i64>>,
    pub root_indptr: Arc<Vec<u64>>,
    pub root_indices: Arc<Vec<u64>>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Concepts {
    pub metadata_json: String,
    pub vectors: Option<Matrix>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceFacts {
    pub metadata_json: String,
    pub sequence_roots: Arc<Vec<u64>>,
    pub indptr: Arc<Vec<u64>>,
    pub indices: Arc<Vec<u64>>,
    pub positions: Arc<Vec<i64>>,
    pub ordered: Arc<Vec<bool>>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Associations {
    pub metadata_json: String,
    pub sources: Arc<Vec<u64>>,
    pub targets: Arc<Vec<u64>>,
    pub directed: Arc<Vec<bool>>,
    pub strengths: Arc<Vec<f64>>,
    pub confidences: Arc<Vec<f64>>,
    pub polarities: Arc<Vec<i8>>,
    pub edge_groups: Arc<Vec<u64>>,
    pub group_root_indptr: Arc<Vec<u64>>,
    pub group_root_indices: Arc<Vec<u64>>,
}
impl Matrix {
    pub fn row(&self, row: usize) -> &[f32] {
        &self.values[row * self.cols..(row + 1) * self.cols]
    }
}
pub fn csr(ptr: &[u64], indices: &[u64], rows: usize, bound: usize) -> Result<()> {
    ensure!(
        ptr.len() == rows + 1
            && ptr.first() == Some(&0)
            && ptr.last() == Some(&(indices.len() as u64))
            && ptr.windows(2).all(|p| p[0] <= p[1])
            && indices.iter().all(|x| *x < bound as u64),
        "Invalid CSR offsets or row indices"
    );
    Ok(())
}
