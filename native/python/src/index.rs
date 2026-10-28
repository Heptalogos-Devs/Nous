use crate::{CancellationToken, native_error};
use anyhow::{Context, Result, ensure};
use nous_retrieval::{
    IndexDocument, RetrievalIndex,
    serving::{
        ServingCandidate, ServingGeneration, ServingGraph, ServingProfile, ServingSnapshot,
        ServingTag, VcpConfig,
    },
    vector::{VectorGeneration, VectorGenerationIdentity, VectorRecord, VectorSnapshot},
};
use numpy::{IntoPyArray, PyArray1, PyReadonlyArray1, PyReadonlyArray2, PyUntypedArrayMethods};
use pyo3::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

mod config;
pub use config::schemas;
use config::*;

mod store;
use store::*;
mod query;

#[pyclass(frozen, module = "nous_native._core")]
pub struct IndexStore {
    state: Arc<StoreState>,
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone)]
pub struct Snapshot {
    inner: Arc<Frozen>,
    state: Arc<StoreState>,
}
#[pyclass(frozen, module = "nous_native._core")]
pub struct SearchResult {
    rows: Vec<u64>,
    scores: Vec<f64>,
    #[pyo3(get)]
    generation: String,
    #[pyo3(get)]
    diagnostics_json: String,
}
#[pymethods]
impl SearchResult {
    #[getter]
    fn row_ids<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<u64>> {
        self.rows.clone().into_pyarray(py)
    }
    #[getter]
    fn scores<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f64>> {
        self.scores.clone().into_pyarray(py)
    }
}
fn array2(value: Option<PyReadonlyArray2<'_, f32>>, name: &str) -> PyResult<Option<Vec<Vec<f32>>>> {
    value
        .map(|array| {
            if !array.is_c_contiguous() {
                return Err(native_error(format!(
                    "{name}: C-contiguous float32 matrix required"
                )));
            }
            let shape = array.shape();
            let width = shape[1];
            if width == 0 {
                return Err(native_error(format!("{name}: dimension must be positive")));
            }
            let slice = array.as_slice().map_err(|_| {
                native_error(format!("{name}: C-contiguous float32 matrix required"))
            })?;
            if slice.iter().any(|x| !x.is_finite()) {
                return Err(native_error(format!("{name}: values must be finite")));
            }
            Ok(slice.chunks(width).map(|row| row.to_vec()).collect())
        })
        .transpose()
}
fn array1(value: Option<PyReadonlyArray1<'_, u64>>, name: &str) -> PyResult<Vec<u64>> {
    value
        .map(|array| {
            array
                .as_slice()
                .map(|v| v.to_vec())
                .map_err(|_| native_error(format!("{name}: contiguous uint64 vector required")))
        })
        .unwrap_or_else(|| Ok(vec![]))
}
#[pymethods]
impl IndexStore {
    #[new]
    #[pyo3(signature=(path,options_json="{}"))]
    fn new(py: Python<'_>, path: String, options_json: &str) -> PyResult<Self> {
        let options: StoreOptions = serde_json::from_str(options_json).map_err(native_error)?;
        if !(1..=64).contains(&options.threads) {
            return Err(native_error("threads must be between 1 and 64"));
        }
        let state = py
            .detach(|| -> Result<StoreState> {
                let path = PathBuf::from(path);
                std::fs::create_dir_all(&path)?;
                Ok(StoreState {
                    path,
                    pool: rayon::ThreadPoolBuilder::new()
                        .num_threads(options.threads)
                        .build()?,
                    closed: AtomicBool::new(false),
                    writer: Mutex::new(()),
                    active: RwLock::new(()),
                })
            })
            .map_err(native_error)?;
        Ok(Self {
            state: Arc::new(state),
        })
    }
    #[pyo3(signature=(*,references,texts,assets_json="{}",cancel=None,vectors=None,tag_vectors=None,tag_indptr=None,tag_indices=None,graph_indptr=None,graph_indices=None,graph_weights=None,graph_polarities=None,source_references=None,candidate_source_indptr=None,candidate_source_indices=None,graph_source_indptr=None,graph_source_indices=None))]
    fn build(
        &self,
        py: Python<'_>,
        references: Vec<String>,
        texts: Vec<String>,
        assets_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
        vectors: Option<PyReadonlyArray2<'_, f32>>,
        tag_vectors: Option<PyReadonlyArray2<'_, f32>>,
        tag_indptr: Option<PyReadonlyArray1<'_, u64>>,
        tag_indices: Option<PyReadonlyArray1<'_, u64>>,
        graph_indptr: Option<PyReadonlyArray1<'_, u64>>,
        graph_indices: Option<PyReadonlyArray1<'_, u64>>,
        graph_weights: Option<PyReadonlyArray1<'_, f64>>,
        graph_polarities: Option<PyReadonlyArray1<'_, i8>>,
        source_references: Option<Vec<String>>,
        candidate_source_indptr: Option<PyReadonlyArray1<'_, u64>>,
        candidate_source_indices: Option<PyReadonlyArray1<'_, u64>>,
        graph_source_indptr: Option<PyReadonlyArray1<'_, u64>>,
        graph_source_indices: Option<PyReadonlyArray1<'_, u64>>,
    ) -> PyResult<Snapshot> {
        let assets: Assets = serde_json::from_str(assets_json).map_err(native_error)?;
        let explicit_sources = source_references.is_some();
        let sources = source_references.unwrap_or_else(|| references.clone());
        if explicit_sources
            && (candidate_source_indptr.is_none() || candidate_source_indices.is_none())
        {
            return Err(native_error(
                "candidate source CSR required with source_references",
            ));
        }
        let candidate_sources = if explicit_sources {
            (
                array1(candidate_source_indptr, "candidate_source_indptr")?,
                array1(candidate_source_indices, "candidate_source_indices")?,
            )
        } else {
            (
                (0..=references.len() as u64).collect(),
                (0..references.len() as u64).collect(),
            )
        };
        let graph_source_rows = array1(graph_source_indptr, "graph_source_indptr")?;
        let graph_source_ids = array1(graph_source_indices, "graph_source_indices")?;
        let has_graph = graph_indptr.is_some();
        if !has_graph
            && (graph_indices.is_some() || graph_weights.is_some() || graph_polarities.is_some())
        {
            return Err(native_error("graph_indptr required for graph arrays"));
        }
        let graph = if has_graph {
            let weights = graph_weights
                .context("graph_weights required")
                .map_err(native_error)?
                .as_slice()
                .map_err(native_error)?
                .to_vec();
            let polarities = graph_polarities
                .map(|values| values.as_slice().map(|v| v.to_vec()).map_err(native_error))
                .transpose()?
                .unwrap_or_else(|| vec![1; weights.len()]);
            Some(ServingGraph {
                tag_keys: assets.tags.iter().map(|tag| tag.key.clone()).collect(),
                indptr: array1(graph_indptr, "graph_indptr")?,
                indices: array1(graph_indices, "graph_indices")?,
                source_references: sources.clone(),
                source_indptr: if graph_source_rows.is_empty() {
                    vec![0; weights.len() + 1]
                } else {
                    graph_source_rows
                },
                source_indices: graph_source_ids,
                polarities,
                weights,
            })
        } else if !assets.tags.is_empty() {
            Some(ServingGraph {
                tag_keys: assets.tags.iter().map(|tag| tag.key.clone()).collect(),
                indptr: vec![0; assets.tags.len() + 1],
                indices: vec![],
                weights: vec![],
                polarities: vec![],
                source_references: sources.clone(),
                source_indptr: vec![0],
                source_indices: vec![],
            })
        } else {
            None
        };
        let data = BuildData {
            references,
            texts,
            assets,
            graph,
            source_references: sources,
            candidate_source_indptr: candidate_sources.0,
            candidate_source_indices: candidate_sources.1,
            vectors: array2(vectors, "vectors")?,
            tag_vectors: array2(tag_vectors, "tag_vectors")?,
            tag_indptr: array1(tag_indptr, "tag_indptr")?,
            tag_indices: array1(tag_indices, "tag_indices")?,
        };
        let token = cancel
            .map(|value| value.clone())
            .unwrap_or_else(|| CancellationToken {
                inner: Default::default(),
            });
        let state = self.state.clone();
        let inner = py
            .detach(|| state.pool.install(|| build(&state, data, &token)))
            .map_err(native_error)?;
        Ok(Snapshot {
            inner: Arc::new(inner),
            state,
        })
    }
    fn open_snapshot(&self, py: Python<'_>, generation: String) -> PyResult<Snapshot> {
        let state = self.state.clone();
        let inner = py
            .detach(|| {
                state.pool.install(|| -> Result<Frozen> {
                    let _active = state
                        .active
                        .read()
                        .map_err(|_| anyhow::anyhow!("native lifecycle poisoned"))?;
                    ensure!(!state.closed.load(Ordering::Acquire), "index store closed");
                    validate_generation(&generation)?;
                    load(&state.path.join(&generation), generation)
                })
            })
            .map_err(native_error)?;
        Ok(Snapshot {
            inner: Arc::new(inner),
            state,
        })
    }
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| -> Result<()> {
            self.state.closed.store(true, Ordering::Release);
            let _active = self
                .state
                .active
                .write()
                .map_err(|_| anyhow::anyhow!("native lifecycle poisoned"))?;
            Ok(())
        })
        .map_err(native_error)
    }
}
#[pymethods]
impl Snapshot {
    #[getter]
    fn generation(&self) -> &str {
        &self.inner.generation
    }
    #[getter]
    fn capabilities(&self) -> Vec<&'static str> {
        let mut modes = vec!["lexical"];
        if self.inner.dense.is_some() {
            modes.extend(["dense", "hybrid"])
        }
        if self.inner.vcp.is_some() {
            modes.push("vcp")
        }
        modes
    }
    #[getter]
    fn references(&self) -> Vec<String> {
        self.inner.data.references.clone()
    }
    #[pyo3(signature=(*,mode,text="",vectors=None,allowed_rows,allowed_graph_nodes=None,options_json="{}",cancel=None,ghost_vectors=None))]
    fn query(
        &self,
        py: Python<'_>,
        mode: String,
        text: &str,
        vectors: Option<PyReadonlyArray1<'_, f32>>,
        allowed_rows: PyReadonlyArray1<'_, u64>,
        allowed_graph_nodes: Option<PyReadonlyArray1<'_, u64>>,
        options_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
        ghost_vectors: Option<PyReadonlyArray2<'_, f32>>,
    ) -> PyResult<SearchResult> {
        let query = vectors
            .map(|array| {
                array
                    .as_slice()
                    .map(|v| v.to_vec())
                    .map_err(|_| native_error("query vectors require contiguous float32"))
            })
            .transpose()?;
        let allowed = allowed_rows
            .as_slice()
            .map_err(native_error)?
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let graph = allowed_graph_nodes
            .map(|array| {
                array
                    .as_slice()
                    .map(|v| v.iter().copied().collect::<BTreeSet<_>>())
                    .map_err(native_error)
            })
            .transpose()?;
        let options: QueryOptions = serde_json::from_str(options_json).map_err(native_error)?;
        let ghosts = array2(ghost_vectors, "ghost_vectors")?;
        let token = cancel
            .map(|value| value.clone())
            .unwrap_or_else(|| CancellationToken {
                inner: Default::default(),
            });
        py.detach(|| {
            self.state.pool.install(|| {
                let _active = self
                    .state
                    .active
                    .read()
                    .map_err(|_| anyhow::anyhow!("native lifecycle poisoned"))?;
                ensure!(
                    !self.state.closed.load(Ordering::Acquire),
                    "index store closed"
                );
                self.inner.query(
                    &mode,
                    &text,
                    query.as_deref(),
                    &allowed,
                    graph.as_ref(),
                    options,
                    ghosts,
                    &token,
                )
            })
        })
        .map_err(native_error)
    }
}
