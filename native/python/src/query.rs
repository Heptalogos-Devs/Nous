//! Thin Python observation and result handles.
use crate::*;
use anyhow::Result;
use nous_retrieval::query::{Options, ThinkingStage};
use numpy::{PyArray2, PyReadonlyArray1, PyReadonlyArray2, PyUntypedArrayMethods};
use serde_json::Value;
use std::collections::BTreeSet;
#[pyclass(frozen, module = "nous_native._core")]
pub struct CandidateSet {
    inner: nous_retrieval::query::CandidateSet,
}
#[pymethods]
impl CandidateSet {
    #[new]
    fn new(value_json: &str) -> PyResult<Self> {
        Ok(Self {
            inner: serde_json::from_str(value_json)
                .map_err(|e| native_error(failure("wave.invalid_input", e)))?,
        })
    }
    #[getter]
    fn value_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner).map_err(|e| native_error(anyhow::Error::from(e)))
    }
}
#[pyclass(frozen, module = "nous_native._core")]
pub struct QueryObservation {
    pub(crate) inner: nous_retrieval::query::QueryObservation,
}
#[pymethods]
impl QueryObservation {
    #[getter]
    fn diagnostics_json(&self) -> String {
        self.inner.diagnostics_json.clone()
    }
}
fn options(raw: &str) -> PyResult<Options> {
    serde_json::from_str(raw).map_err(|e| native_error(failure("wave.invalid_input", e)))
}
fn array(v: Option<PyReadonlyArray1<'_, f32>>) -> PyResult<Option<Vec<f32>>> {
    v.map(|v| {
        v.as_slice()
            .map(|v| v.to_vec())
            .map_err(|e| native_error(failure("wave.invalid_input", e)))
    })
    .transpose()
}
fn ghosts(v: Option<PyReadonlyArray2<'_, f32>>) -> PyResult<Vec<Vec<f32>>> {
    v.map(|v| {
        let shape = v.shape();
        let width = shape[1];
        if width == 0 {
            return Err(native_error(failure(
                "wave.invalid_input",
                "Ghost vector dimension must be positive",
            )));
        }
        Ok(v.as_slice()
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?
            .chunks(width)
            .map(|r| r.to_vec())
            .collect())
    })
    .unwrap_or(Ok(vec![]))
}
fn allowed(v: Option<PyReadonlyArray1<'_, u64>>, count: usize) -> PyResult<BTreeSet<u64>> {
    v.map(|v| {
        v.as_slice()
            .map(|v| v.iter().copied().collect())
            .map_err(|e| native_error(failure("wave.invalid_input", e)))
    })
    .unwrap_or(Ok((0..count as u64).collect()))
}
impl NativeSnapshot {
    pub(crate) fn run<T: Send>(
        &self,
        py: Python<'_>,
        token: &CancellationToken,
        operation: impl FnOnce() -> Result<T> + Send,
    ) -> PyResult<T> {
        py.detach(|| self.state.run(&token.inner, operation))
            .map_err(native_error)
    }
}
#[pymethods]
impl NativeSnapshot {
    #[pyo3(signature=(rows, *, references=None, aspects=None, cancel=None))]
    fn passage_similarities<'py>(
        &self,
        py: Python<'py>,
        rows: PyReadonlyArray1<'_, u64>,
        references: Option<PyReadonlyArray1<'_, u64>>,
        aspects: Option<PyReadonlyArray2<'_, f32>>,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<Bound<'py, PyArray2<f64>>> {
        let rows = rows
            .as_slice()
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?
            .to_vec();
        let references = references
            .map(|value| value.as_slice().map(|items| items.to_vec()))
            .transpose()
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?
            .unwrap_or_default();
        let aspects = ghosts(aspects)?;
        let token = cancel.map(|value| value.clone()).unwrap_or_default();
        let values = self.run(py, &token, || {
            self.inner
                .passage_similarities(&rows, &references, &aspects, &token.inner)
        })?;
        PyArray2::from_vec2(py, &values)
            .map_err(|error| native_error(failure("wave.invalid_input", error)))
    }

    #[getter]
    fn identity_json(&self) -> String {
        self.inner.inner.input.identity.to_string()
    }
    #[getter]
    pub(crate) fn capabilities(&self) -> Vec<&'static str> {
        let mut result = vec!["text"];
        if self.inner.inner.ann.is_some() {
            result.push("vectors")
        }
        if self.inner.inner.assets.is_some() {
            result.extend(["vcp", "thinking"])
        }
        result
    }
    #[getter]
    fn statistics_json(&self) -> String {
        self.inner.inner.stats.to_string()
    }
    #[pyo3(signature=(vector,*,text="",options_json="{}",ghost_vectors=None,cancel=None))]
    fn observe(
        &self,
        py: Python<'_>,
        vector: PyReadonlyArray1<'_, f32>,
        text: &str,
        options_json: &str,
        ghost_vectors: Option<PyReadonlyArray2<'_, f32>>,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<QueryObservation> {
        let vector = array(Some(vector))?.unwrap();
        let o = options(options_json)?;
        let ghosts = ghosts(ghost_vectors)?;
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        self.run(py, &token, || {
            self.inner
                .observation(&vector, text, &o, ghosts, &token.inner)
                .map(|inner| QueryObservation { inner })
        })
    }
    #[pyo3(signature=(*,name,program_json,expression_json=None,text="",raw_vector=None,contextual_vector=None,branch_raw_vectors=None,branch_contextual_vectors=None,allowed_rows=None,options_json="{}",ghost_vectors=None,candidate_set=None,cancel=None))]
    #[allow(clippy::too_many_arguments)]
    fn execute(
        &self,
        py: Python<'_>,
        name: &str,
        program_json: &str,
        expression_json: Option<&str>,
        text: &str,
        raw_vector: Option<PyReadonlyArray1<'_, f32>>,
        contextual_vector: Option<PyReadonlyArray1<'_, f32>>,
        branch_raw_vectors: Option<PyReadonlyArray2<'_, f32>>,
        branch_contextual_vectors: Option<PyReadonlyArray2<'_, f32>>,
        allowed_rows: Option<PyReadonlyArray1<'_, u64>>,
        options_json: &str,
        ghost_vectors: Option<PyReadonlyArray2<'_, f32>>,
        candidate_set: Option<PyRef<'_, CandidateSet>>,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<String> {
        let raw = array(raw_vector)?;
        let contextual = array(contextual_vector)?;
        let branch_raw = ghosts(branch_raw_vectors)?;
        let branch_contextual = ghosts(branch_contextual_vectors)?;
        let allowed = allowed(allowed_rows, self.inner.inner.candidates.keys.len())?;
        let options = if expression_json.is_none() {
            Some(options(options_json)?)
        } else {
            None
        };
        let ghosts = ghosts(ghost_vectors)?;
        let token = cancel.map(|value| value.clone()).unwrap_or_default();
        let candidate_set = candidate_set.as_deref();
        let value = self.run(py, &token, || {
            if let Some(expression) = expression_json {
                anyhow::ensure!(
                    candidate_set.is_none(),
                    "Fixed candidates cannot be combined with an expression"
                );
                self.inner.execute_expression(
                    name,
                    program_json,
                    expression,
                    text,
                    raw.as_deref(),
                    &branch_raw,
                    &branch_contextual,
                    &allowed,
                    options_json,
                    ghosts,
                    &token.inner,
                )
            } else {
                self.inner.execute_program(
                    name,
                    program_json,
                    text,
                    raw.as_deref(),
                    contextual.as_deref(),
                    &allowed,
                    options.unwrap(),
                    ghosts,
                    candidate_set.map(|pool| &pool.inner),
                    &token.inner,
                )
            }
        })?;
        serde_json::to_string(&value).map_err(native_error)
    }
    #[pyo3(signature=(observation,stages_json,*,options_json="{}",cancel=None))]
    fn think(
        &self,
        py: Python<'_>,
        observation: PyRef<'_, QueryObservation>,
        stages_json: &str,
        options_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<String> {
        let options = options(options_json)?;
        let stages: Vec<ThinkingStage> = serde_json::from_str(stages_json)
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?;
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        let observation = &*observation;
        self.run(py, &token, || {
            self.inner.validate_observation(&observation.inner)?;
            Ok(self
                .inner
                .thinking(
                    &observation.inner,
                    &stages,
                    options.thinking_min_closure,
                    &(0..self.inner.inner.candidates.keys.len() as u64).collect(),
                    &token.inner,
                )?
                .to_string())
        })
    }
    #[pyo3(signature=(rows,*,options_json="{}",cancel=None))]
    fn neighborhood(
        &self,
        py: Python<'_>,
        rows: PyReadonlyArray1<'_, u64>,
        options_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<String> {
        let rows = rows
            .as_slice()
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?
            .to_vec();
        let options: Value = serde_json::from_str(options_json)
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?;
        let limit = options.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        self.run(py, &token, || {
            self.inner.neighborhood(rows, limit, &token.inner)
        })
    }
}
