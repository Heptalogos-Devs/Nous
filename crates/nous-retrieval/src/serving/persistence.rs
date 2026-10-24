//! Atomic grouped publication, checksums and graph recovery.
use super::*;

impl ServingGeneration {
    pub fn create_or_open(parent: &Path, snapshot: &ServingSnapshot) -> Result<Self> {
        ensure!(
            parent.is_dir(),
            "SERVING_PATH_INVALID: managed parent must exist"
        );
        let prepared = prepare(snapshot)?;
        let path = parent.join(&prepared.identity.generation);
        if path.try_exists()? {
            return Self::open(&path, &prepared);
        }
        let staging = tempfile::Builder::new()
            .prefix(".nous-serving-")
            .tempdir_in(parent)?;
        let (assets, geometry, graph, artifact_meta) = build_assets(&prepared)?;
        let ann = VectorGeneration::create(&staging.path().join("ann"), prepared.vectors.clone())?;
        let geometry_hash = write_json(&staging.path().join("geometry.json"), &geometry)?;
        let graph_hash = write_json(&staging.path().join("graph.json"), &graph)?;
        let mappings_hash = write_json(&staging.path().join("mappings.json"), &prepared.mappings)?;
        let manifest = ServingManifest {
            digest: prepared.digest.clone(),
            identity: prepared.identity.clone(),
            geometry_hash,
            graph_hash,
            mappings_hash,
            artifact_meta,
        };
        write_json(&staging.path().join("manifest.json"), &manifest)?;
        sync_dir(staging.path())?;
        fs::rename(staging.path(), &path)
            .context("SERVING_PUBLISH_FAILED: complete generation rename")?;
        sync_dir(parent)?;
        let asset_diagnostics = asset_diagnostics(&assets, &prepared.mappings);
        Ok(Self {
            identity: prepared.identity,
            assets,
            ann,
            mappings: prepared.mappings,
            path,
            restored: false,
            asset_diagnostics,
        })
    }
    pub(super) fn open(path: &Path, prepared: &Prepared) -> Result<Self> {
        let manifest: ServingManifest = read_json(&path.join("manifest.json"))?;
        ensure!(
            manifest.digest == prepared.digest && manifest.identity == prepared.identity,
            "SERVING_MANIFEST_MISMATCH: frozen snapshot or format changed"
        );
        let geometry: Value = read_verified(&path.join("geometry.json"), &manifest.geometry_hash)?;
        let graph: GraphDisk = read_verified(&path.join("graph.json"), &manifest.graph_hash)?;
        let mappings: Mappings =
            read_verified(&path.join("mappings.json"), &manifest.mappings_hash)?;
        ensure!(
            mappings == prepared.mappings,
            "SERVING_MAPPING_MISMATCH: frozen source identities changed"
        );
        let graph = graph.restore()?;
        let assets = assemble_assets(prepared, graph, &geometry, &manifest.artifact_meta)?;
        let ann = VectorGeneration::open(&path.join("ann"), &prepared.vectors)?;
        let asset_diagnostics = asset_diagnostics(&assets, &mappings);
        Ok(Self {
            identity: prepared.identity.clone(),
            assets,
            ann,
            mappings,
            path: path.into(),
            restored: true,
            asset_diagnostics,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct ServingManifest {
    pub(super) digest: String,
    pub(super) identity: VectorGenerationIdentity,
    pub(super) geometry_hash: String,
    pub(super) graph_hash: String,
    pub(super) mappings_hash: String,
    pub(super) artifact_meta: Value,
}
#[derive(Serialize, Deserialize)]
pub(super) struct GraphDisk {
    pub(super) nodes: Vec<i64>,
    pub(super) offsets: Vec<usize>,
    pub(super) targets: Vec<usize>,
    pub(super) weights: Vec<f64>,
    pub(super) inbound: BTreeMap<i64, f64>,
    pub(super) max_inbound: f64,
    pub(super) anchor_gain: BTreeMap<i64, f64>,
    pub(super) wormholes: BTreeSet<(i64, i64)>,
    pub(super) provenance: Vec<EdgeProvenance>,
}
pub(super) type EdgeProvenance = ((i64, i64), Vec<(i64, f64)>);
impl GraphDisk {
    pub(super) fn capture(graph: &NativeArtifact) -> Self {
        let mut provenance = graph
            .provenance
            .iter()
            .map(|(edge, values)| (*edge, values.clone()))
            .collect::<Vec<_>>();
        provenance.sort_by_key(|item| item.0);
        Self {
            nodes: graph.node_ids.clone(),
            offsets: graph.row_offsets.clone(),
            targets: graph.targets.clone(),
            weights: graph.weights.clone(),
            inbound: graph.inbound.iter().map(|(a, b)| (*a, *b)).collect(),
            max_inbound: graph.max_inbound,
            anchor_gain: graph.anchor_gain.iter().map(|(a, b)| (*a, *b)).collect(),
            wormholes: graph.wormhole_edges.iter().copied().collect(),
            provenance,
        }
    }
    pub(super) fn restore(self) -> Result<Arc<NativeArtifact>> {
        let graph = NativeArtifact {
            node_index: self
                .nodes
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i))
                .collect(),
            node_ids: self.nodes,
            row_offsets: self.offsets,
            targets: self.targets,
            weights: self.weights,
            inbound: self.inbound.into_iter().collect(),
            max_inbound: self.max_inbound,
            anchor_gain: self.anchor_gain.into_iter().collect(),
            wormhole_edges: self.wormholes.into_iter().collect(),
            provenance: self.provenance.into_iter().collect(),
            ..Default::default()
        }
        .try_prepared()
        .map_err(anyhow::Error::msg)?;
        Ok(Arc::new(graph))
    }
}
pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    let mut file = File::create(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(hash(&bytes))
}
pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(
        File::open(path)
            .with_context(|| format!("SERVING_GENERATION_INCOMPLETE: {}", path.display()))?,
    )
    .context("SERVING_COMPONENT_INVALID")
}
pub(super) fn read_verified<T: serde::de::DeserializeOwned>(
    path: &Path,
    expected: &str,
) -> Result<T> {
    let mut bytes = Vec::new();
    File::open(path)
        .with_context(|| format!("SERVING_GENERATION_INCOMPLETE: {}", path.display()))?
        .read_to_end(&mut bytes)?;
    ensure!(
        hash(&bytes) == expected,
        "SERVING_CHECKSUM_MISMATCH: changed component"
    );
    serde_json::from_slice(&bytes).context("SERVING_COMPONENT_INVALID")
}
pub(super) fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(not(windows))]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}
