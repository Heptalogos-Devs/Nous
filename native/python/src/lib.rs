use pyo3::prelude::*;
mod bundles;
mod context;
mod engine;
mod query;
pub(crate) use bundles::*;
pub(crate) use engine::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pyo3::create_exception!(_core, NativeError, pyo3::exceptions::PyRuntimeError);
#[derive(Debug)]
pub(crate) struct Failure {
    pub code: &'static str,
    pub message: String,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Failure {}
pub(crate) fn failure(code: &'static str, message: impl ToString) -> anyhow::Error {
    Failure {
        code,
        message: message.to_string(),
    }
    .into()
}
pub(crate) fn native_error(error: impl Into<anyhow::Error>) -> PyErr {
    let error = error.into();
    let code = error
        .downcast_ref::<Failure>()
        .map_or("wave.native_error", |e| e.code);
    Python::attach(|py| {
        let result = NativeError::new_err(format!("{error:#}"));
        let value = result.value(py);
        let _ = value.setattr("code", code);
        let _ = value.setattr("details_json", "{}");
        result
    })
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone, Default)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
}
impl CancellationToken {
    pub(crate) fn check(&self) -> anyhow::Result<()> {
        if self.inner.load(Ordering::Relaxed) {
            Err(failure("wave.cancelled", "Native operation cancelled"))
        } else {
            Ok(())
        }
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::Relaxed)
    }
}
#[pymethods]
impl CancellationToken {
    #[new]
    fn new() -> Self {
        Self::default()
    }
    fn cancel(&self) {
        self.inner.store(true, Ordering::Relaxed)
    }
    #[getter]
    fn cancelled(&self) -> bool {
        self.is_cancelled()
    }
}
#[pyfunction]
fn schemas_json() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/schemas.json"))
}
#[pyfunction]
fn defaults_json() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/defaults.json"))
}
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("NativeError", m.py().get_type::<NativeError>())?;
    m.add_class::<CancellationToken>()?;
    m.add_class::<Candidates>()?;
    m.add_class::<Concepts>()?;
    m.add_class::<SourceFacts>()?;
    m.add_class::<Associations>()?;
    m.add_class::<NativeEngine>()?;
    m.add_class::<NativeSnapshot>()?;
    m.add_class::<query::QueryObservation>()?;
    m.add_class::<query::NativeQueryResult>()?;
    m.add_function(wrap_pyfunction!(schemas_json, m)?)?;
    m.add_function(wrap_pyfunction!(defaults_json, m)?)?;
    m.add_function(wrap_pyfunction!(context::aggregate_context, m)?)?;
    m.add_function(wrap_pyfunction!(context::segment, m)?)?;
    m.add_class::<context::ContextResult>()?;
    m.add_class::<context::SegmentResult>()?;
    Ok(())
}
