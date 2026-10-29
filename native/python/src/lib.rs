use pyo3::prelude::*;
mod index;
use index::*;

pyo3::create_exception!(_core, NativeError, pyo3::exceptions::PyRuntimeError);
pyo3::create_exception!(_core, NousQLSyntaxError, NativeError);

pub fn native_error(error: impl std::fmt::Display) -> PyErr {
    let message = format!("{error:#}");
    let code = if message.starts_with("CANCELLED") {
        "cancelled"
    } else {
        "native_error"
    };
    Python::attach(|py| {
        let err = NativeError::new_err(message);
        let value = err.value(py);
        let _ = value.setattr("code", code);
        let _ = value.setattr("details_json", "{}");
        err
    })
}

#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone)]
pub struct CancellationToken {
    pub(crate) inner: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl CancellationToken {
    pub(crate) fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.inner.load(std::sync::atomic::Ordering::Relaxed),
            "CANCELLED: native operation cancelled"
        );
        Ok(())
    }
}
#[pymethods]
impl CancellationToken {
    #[new]
    fn new() -> Self {
        Self {
            inner: Default::default(),
        }
    }
    fn cancel(&self) {
        self.inner.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    #[getter]
    fn cancelled(&self) -> bool {
        self.inner.load(std::sync::atomic::Ordering::Relaxed)
    }
}
#[pyclass(frozen, module = "nous_native._core")]
pub struct ParsedQuery {
    #[pyo3(get)]
    ast_json: String,
    #[pyo3(get)]
    canonical: String,
}
#[pyfunction]
fn parse_nousql(py: Python<'_>, source: String) -> PyResult<ParsedQuery> {
    let parsed = py.detach(|| nousql_syntax::parse(&source));
    match parsed {
        Ok(ast) => Ok(ParsedQuery {
            canonical: ast.canonical(),
            ast_json: serde_json::to_string(&ast).map_err(native_error)?,
        }),
        Err(error) => {
            let err = NousQLSyntaxError::new_err(error.message);
            let value = err.value(py);
            value.setattr("code", "nousql_syntax")?;
            value.setattr("details_json", "{}")?;
            value.setattr("start_byte", error.start_byte)?;
            value.setattr("end_byte", error.end_byte)?;
            Err(err)
        }
    }
}
#[pyfunction]
fn nousql_manual() -> &'static str {
    nousql_syntax::MANUAL
}
#[pyfunction]
fn schemas_json() -> String {
    index::schemas().to_string()
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("NativeError", m.py().get_type::<NativeError>())?;
    m.add("NousQLSyntaxError", m.py().get_type::<NousQLSyntaxError>())?;
    m.add_class::<CancellationToken>()?;
    m.add_class::<ParsedQuery>()?;
    m.add_class::<IndexStore>()?;
    m.add_class::<Snapshot>()?;
    m.add_class::<SearchResult>()?;
    m.add_function(wrap_pyfunction!(parse_nousql, m)?)?;
    m.add_function(wrap_pyfunction!(nousql_manual, m)?)?;
    m.add_function(wrap_pyfunction!(schemas_json, m)?)?;
    Ok(())
}
