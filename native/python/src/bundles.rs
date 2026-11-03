//! Owned array bundles: all NumPy borrows end before Python::detach.
use crate::native_error;
use anyhow::{Result, ensure};
use numpy::{PyReadonlyArray1, PyReadonlyArray2, PyUntypedArrayMethods};
use pyo3::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Clone, Serialize, Deserialize, Default)]
pub(crate) struct Matrix {
    pub values: Arc<Vec<f32>>,
    pub rows: usize,
    pub cols: usize,
}
impl Matrix {
    pub fn row(&self, row: usize) -> &[f32] {
        &self.values[row * self.cols..(row + 1) * self.cols]
    }
}
fn matrix(value: Option<PyReadonlyArray2<'_, f32>>) -> PyResult<Option<Matrix>> {
    value
        .map(|v| {
            if !v.is_c_contiguous() {
                return Err(native_error(anyhow::anyhow!(
                    "C-contiguous float32 matrix required"
                )));
            }
            let shape = v.shape();
            let values = v
                .as_slice()
                .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                .to_vec();
            if values.iter().any(|x| !x.is_finite()) || shape[1] == 0 {
                return Err(native_error(anyhow::anyhow!(
                    "Finite vectors and positive dimensions required"
                )));
            }
            Ok(Matrix {
                values: Arc::new(values),
                rows: shape[0],
                cols: shape[1],
            })
        })
        .transpose()
}
pub(crate) fn csr(ptr: &[u64], indices: &[u64], rows: usize, bound: usize) -> Result<()> {
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
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone, Serialize, Deserialize)]
pub struct Candidates {
    pub(crate) metadata_json: String,
    pub(crate) texts: Arc<Vec<String>>,
    pub(crate) vectors: Option<Matrix>,
    pub(crate) membership_indptr: Arc<Vec<u64>>,
    pub(crate) membership_indices: Arc<Vec<u64>>,
    pub(crate) curve_indptr: Arc<Vec<u64>>,
    pub(crate) curve_indices: Arc<Vec<u64>>,
    pub(crate) curve_positions: Arc<Vec<i64>>,
    pub(crate) root_indptr: Arc<Vec<u64>>,
    pub(crate) root_indices: Arc<Vec<u64>>,
}
#[pymethods]
impl Candidates {
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(
        metadata_json: String,
        texts: Vec<String>,
        vectors: Option<PyReadonlyArray2<'_, f32>>,
        membership_indptr: PyReadonlyArray1<'_, u64>,
        membership_indices: PyReadonlyArray1<'_, u64>,
        curve_indptr: PyReadonlyArray1<'_, u64>,
        curve_indices: PyReadonlyArray1<'_, u64>,
        curve_positions: PyReadonlyArray1<'_, i64>,
        root_indptr: PyReadonlyArray1<'_, u64>,
        root_indices: PyReadonlyArray1<'_, u64>,
    ) -> PyResult<Self> {
        let _: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        Ok(Self {
            metadata_json,
            texts: Arc::new(texts),
            vectors: matrix(vectors)?,
            membership_indptr: Arc::new(
                membership_indptr
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            membership_indices: Arc::new(
                membership_indices
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            curve_indptr: Arc::new(
                curve_indptr
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            curve_indices: Arc::new(
                curve_indices
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            curve_positions: Arc::new(
                curve_positions
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            root_indptr: Arc::new(
                root_indptr
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            root_indices: Arc::new(
                root_indices
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
        })
    }
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone, Serialize, Deserialize)]
pub struct Concepts {
    pub(crate) metadata_json: String,
    pub(crate) vectors: Option<Matrix>,
}
#[pymethods]
impl Concepts {
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(metadata_json: String, vectors: Option<PyReadonlyArray2<'_, f32>>) -> PyResult<Self> {
        let _: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        Ok(Self {
            metadata_json,
            vectors: matrix(vectors)?,
        })
    }
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone, Serialize, Deserialize)]
pub struct SourceFacts {
    pub(crate) metadata_json: String,
    pub(crate) sequence_roots: Arc<Vec<u64>>,
    pub(crate) indptr: Arc<Vec<u64>>,
    pub(crate) indices: Arc<Vec<u64>>,
    pub(crate) positions: Arc<Vec<i64>>,
    pub(crate) ordered: Arc<Vec<bool>>,
}
#[pymethods]
impl SourceFacts {
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(
        metadata_json: String,
        sequence_roots: PyReadonlyArray1<'_, u64>,
        indptr: PyReadonlyArray1<'_, u64>,
        indices: PyReadonlyArray1<'_, u64>,
        positions: PyReadonlyArray1<'_, i64>,
        ordered: PyReadonlyArray1<'_, bool>,
    ) -> PyResult<Self> {
        let _: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        Ok(Self {
            metadata_json,
            sequence_roots: Arc::new(
                sequence_roots
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            indptr: Arc::new(
                indptr
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            indices: Arc::new(
                indices
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            positions: Arc::new(
                positions
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            ordered: Arc::new(
                ordered
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
        })
    }
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone, Serialize, Deserialize)]
pub struct Associations {
    pub(crate) metadata_json: String,
    pub(crate) sources: Arc<Vec<u64>>,
    pub(crate) targets: Arc<Vec<u64>>,
    pub(crate) directed: Arc<Vec<bool>>,
    pub(crate) strengths: Arc<Vec<f64>>,
    pub(crate) confidences: Arc<Vec<f64>>,
    pub(crate) polarities: Arc<Vec<i8>>,
    pub(crate) root_indptr: Arc<Vec<u64>>,
    pub(crate) root_indices: Arc<Vec<u64>>,
}
#[pymethods]
impl Associations {
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(
        metadata_json: String,
        sources: PyReadonlyArray1<'_, u64>,
        targets: PyReadonlyArray1<'_, u64>,
        directed: PyReadonlyArray1<'_, bool>,
        strengths: PyReadonlyArray1<'_, f64>,
        confidences: PyReadonlyArray1<'_, f64>,
        polarities: PyReadonlyArray1<'_, i8>,
        root_indptr: PyReadonlyArray1<'_, u64>,
        root_indices: PyReadonlyArray1<'_, u64>,
    ) -> PyResult<Self> {
        let _: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        Ok(Self {
            metadata_json,
            sources: Arc::new(
                sources
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            targets: Arc::new(
                targets
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            directed: Arc::new(
                directed
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            strengths: Arc::new(
                strengths
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            confidences: Arc::new(
                confidences
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            polarities: Arc::new(
                polarities
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            root_indptr: Arc::new(
                root_indptr
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
            root_indices: Arc::new(
                root_indices
                    .as_slice()
                    .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                    .to_vec(),
            ),
        })
    }
}
