//! Python array conversion and the bounded native worker lifecycle.
use crate::{bundles::*, *};
use nous_retrieval::{
    engine::{Engine, EngineSettings},
    snapshot::{self, Input},
};
use serde_json::Value;
use std::{path::Path, sync::Arc};
#[pyclass(frozen, module = "nous_native._core")]
pub struct NativeEngine {
    pub(crate) state: Arc<Engine>,
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone)]
pub struct NativeSnapshot {
    pub(crate) inner: nous_retrieval::snapshot::Snapshot,
    pub(crate) state: Arc<Engine>,
}
#[pymethods]
impl NativeEngine {
    #[new]
    #[pyo3(signature=(settings_json="{}"))]
    fn new(settings_json: &str) -> PyResult<Self> {
        let settings: EngineSettings = serde_json::from_str(settings_json)
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?;
        Ok(Self {
            state: Engine::new(settings)
                .map_err(|e| native_error(failure("wave.invalid_input", e)))?,
        })
    }
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature=(candidates,concepts,facts,associations,identity_json,output_dir,*,options_json="{}",base=None,cancel=None))]
    fn build(
        &self,
        py: Python<'_>,
        candidates: PyRef<'_, Candidates>,
        concepts: PyRef<'_, Concepts>,
        facts: PyRef<'_, SourceFacts>,
        associations: PyRef<'_, Associations>,
        identity_json: &str,
        output_dir: String,
        options_json: &str,
        base: Option<PyRef<'_, NativeSnapshot>>,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<String> {
        let input = Input {
            candidates: candidates.inner.clone(),
            concepts: concepts.inner.clone(),
            facts: facts.inner.clone(),
            associations: associations.inner.clone(),
            identity: serde_json::from_str(identity_json)
                .map_err(|e| native_error(anyhow::Error::from(e)))?,
            options: serde_json::from_str(options_json)
                .map_err(|e| native_error(anyhow::Error::from(e)))?,
        };
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        let base = base.map(|v| v.inner.inner.clone());
        let state = self.state.clone();
        let frozen = py
            .detach(|| {
                state.run(&token.inner, || {
                    snapshot::build(input, Path::new(&output_dir), base.as_deref(), &token.inner)
                })
            })
            .map_err(native_error)?;
        serde_json::to_string(&frozen).map_err(|e| native_error(anyhow::Error::from(e)))
    }
    #[pyo3(signature=(path,expected_identity_json="{}",*,cancel=None))]
    fn open(
        &self,
        py: Python<'_>,
        path: String,
        expected_identity_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<NativeSnapshot> {
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        let expected: Value = serde_json::from_str(expected_identity_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        let state = self.state.clone();
        let frozen = py
            .detach(|| {
                state.run(&token.inner, || {
                    snapshot::Snapshot::open(Path::new(&path), &expected, &token.inner)
                })
            })
            .map_err(|e| {
                native_error(if e.is::<Failure>() {
                    e
                } else {
                    failure("wave.snapshot_invalid", e)
                })
            })?;
        Ok(NativeSnapshot {
            inner: frozen,
            state,
        })
    }
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.state.close()).map_err(native_error)
    }
}
