//! Role aggregation and optional adjacent-vector segmentation.
use crate::{failure, native_error};
use nous_vcp::config::ContextControls;
use numpy::{
    IntoPyArray, PyArray1, PyArray2, PyReadonlyArray1, PyReadonlyArray2, PyUntypedArrayMethods,
};
use pyo3::prelude::*;
#[pyfunction]
#[pyo3(signature=(metadata_json,options_json="{}"))]
pub fn context_masses(py: Python<'_>, metadata_json: &str, options_json: &str) -> PyResult<String> {
    py.detach(|| nous_retrieval::context::context_masses(metadata_json, options_json))
        .map_err(|error| native_error(failure("wave.invalid_input", error)))
}
#[pyclass(frozen, module = "nous_native._core")]
pub struct ContextResult {
    vector: Vec<f32>,
    #[pyo3(get)]
    diagnostics_json: String,
}
#[pymethods]
impl ContextResult {
    #[getter]
    fn vector<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f32>> {
        self.vector.clone().into_pyarray(py)
    }
}
#[pyfunction]
#[pyo3(signature=(raw_query,context_vectors,metadata_json,options_json="{}"))]
pub fn aggregate_context(
    py: Python<'_>,
    raw_query: Option<PyReadonlyArray1<'_, f32>>,
    context_vectors: PyReadonlyArray2<'_, f32>,
    metadata_json: &str,
    options_json: &str,
) -> PyResult<ContextResult> {
    let raw = raw_query
        .map(|v| {
            v.as_slice()
                .map(|v| v.to_vec())
                .map_err(|e| native_error(failure("wave.invalid_input", e)))
        })
        .transpose()?;
    let shape = context_vectors.shape();
    let (rows, dim) = (shape[0], shape[1]);
    let vectors = context_vectors
        .as_slice()
        .map_err(|e| native_error(failure("wave.invalid_input", e)))?
        .to_vec();
    let result = py
        .detach(|| {
            nous_retrieval::context::aggregate_context(
                raw,
                vectors,
                rows,
                dim,
                metadata_json,
                options_json,
            )
        })
        .map_err(|e| native_error(failure("wave.invalid_input", e)))?;
    Ok(ContextResult {
        vector: result.vector,
        diagnostics_json: result.diagnostics_json,
    })
}
#[pyclass(frozen, module = "nous_native._core")]
pub struct SegmentResult {
    values: Vec<Vec<f32>>,
    #[pyo3(get)]
    ranges_json: String,
}
#[pymethods]
impl SegmentResult {
    #[getter]
    fn vectors<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray2<f32>>> {
        PyArray2::from_vec2(py, &self.values)
            .map_err(|e| native_error(failure("wave.invalid_input", e)))
    }
}
#[pyfunction]
#[pyo3(signature=(vectors,threshold=None))]
pub fn segment(
    py: Python<'_>,
    vectors: PyReadonlyArray2<'_, f32>,
    threshold: Option<f64>,
) -> PyResult<SegmentResult> {
    let threshold = threshold.unwrap_or_else(|| ContextControls::default().segment_threshold);
    let shape = vectors.shape();
    let (n, d) = (shape[0], shape[1]);
    let values = vectors
        .as_slice()
        .map_err(|e| native_error(failure("wave.invalid_input", e)))?
        .to_vec();
    let result = py
        .detach(|| nous_retrieval::context::segment(values, n, d, threshold))
        .map_err(|e| native_error(failure("wave.invalid_input", e)))?;
    Ok(SegmentResult {
        values: result.values,
        ranges_json: result.ranges_json,
    })
}
