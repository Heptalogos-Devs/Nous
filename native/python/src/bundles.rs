//! Owned array bundles: all NumPy borrows end before Python::detach.
use crate::native_error;
use nous_retrieval::bundles::Matrix;
use numpy::{PyReadonlyArray1, PyReadonlyArray2, PyUntypedArrayMethods};
use pyo3::prelude::*;
use std::sync::Arc;
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
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone)]
pub struct Candidates {
    pub(crate) inner: nous_retrieval::bundles::Candidates,
}
impl std::ops::Deref for Candidates {
    type Target = nous_retrieval::bundles::Candidates;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
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
        path_indptr: PyReadonlyArray1<'_, u64>,
        path_step_indptr: PyReadonlyArray1<'_, u64>,
        path_indices: PyReadonlyArray1<'_, u64>,
        path_positions: PyReadonlyArray1<'_, i64>,
        root_indptr: PyReadonlyArray1<'_, u64>,
        root_indices: PyReadonlyArray1<'_, u64>,
    ) -> PyResult<Self> {
        let _: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        Ok(Self {
            inner: nous_retrieval::bundles::Candidates {
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
                path_indptr: Arc::new(
                    path_indptr
                        .as_slice()
                        .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                        .to_vec(),
                ),
                path_step_indptr: Arc::new(
                    path_step_indptr
                        .as_slice()
                        .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                        .to_vec(),
                ),
                path_indices: Arc::new(
                    path_indices
                        .as_slice()
                        .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                        .to_vec(),
                ),
                path_positions: Arc::new(
                    path_positions
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
            },
        })
    }
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone)]
pub struct Concepts {
    pub(crate) inner: nous_retrieval::bundles::Concepts,
}
impl std::ops::Deref for Concepts {
    type Target = nous_retrieval::bundles::Concepts;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
#[pymethods]
impl Concepts {
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(metadata_json: String, vectors: Option<PyReadonlyArray2<'_, f32>>) -> PyResult<Self> {
        let _: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        Ok(Self {
            inner: nous_retrieval::bundles::Concepts {
                metadata_json,
                vectors: matrix(vectors)?,
            },
        })
    }
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone)]
pub struct SourceFacts {
    pub(crate) inner: nous_retrieval::bundles::SourceFacts,
}
impl std::ops::Deref for SourceFacts {
    type Target = nous_retrieval::bundles::SourceFacts;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
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
            inner: nous_retrieval::bundles::SourceFacts {
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
            },
        })
    }
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone)]
pub struct Associations {
    pub(crate) inner: nous_retrieval::bundles::Associations,
}
impl std::ops::Deref for Associations {
    type Target = nous_retrieval::bundles::Associations;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
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
        edge_groups: PyReadonlyArray1<'_, u64>,
        group_root_indptr: PyReadonlyArray1<'_, u64>,
        group_root_indices: PyReadonlyArray1<'_, u64>,
    ) -> PyResult<Self> {
        let _: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        Ok(Self {
            inner: nous_retrieval::bundles::Associations {
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
                edge_groups: Arc::new(
                    edge_groups
                        .as_slice()
                        .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                        .to_vec(),
                ),
                group_root_indptr: Arc::new(
                    group_root_indptr
                        .as_slice()
                        .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                        .to_vec(),
                ),
                group_root_indices: Arc::new(
                    group_root_indices
                        .as_slice()
                        .map_err(|e| native_error(anyhow::anyhow!(e.to_string())))?
                        .to_vec(),
                ),
            },
        })
    }
}
