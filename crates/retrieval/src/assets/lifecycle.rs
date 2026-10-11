// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{
    assets::build::{DenseManifest, implementation},
    assets::files::*,
    *,
};
use sqlx::Row;
use std::path::Path;

#[derive(Clone)]
pub(crate) enum OpenArtifact {
    Lexical(Arc<LexicalGeneration>),
    Dense(Arc<DenseGeneration>, Option<Arc<EpaBasisGeneration>>),
    Topology(Arc<WaveGraphGeneration>),
    Vcp(Arc<VcpServingGeneration>),
    Concept(Arc<ConceptGeneration>),
}

impl ServingService {
    pub async fn config_digest(
        &self,
        subject: SubjectId,
        family: &str,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> Result<String> {
        let capabilities = self.projection_capabilities(subject).await?;
        let mut config = serde_json::json!({"memory_enabled":capabilities.memory});
        if family == "topology"
            && matches!(
                snapshot.get(nous_runtime::COGNITIVE_PROFILE)?,
                nous_runtime::CognitiveProfile::VcpDtsc
                    | nous_runtime::CognitiveProfile::VcpRiverMemo
            )
        {
            let provider = self.embedding().ok_or_else(|| {
                Error::Unavailable(
                    "VCP serving build requires configured embedding material".into(),
                )
            })?;
            config["space"] = serde_json::to_value(provider.space())
                .map_err(|e| Error::Infrastructure(e.to_string()))?;
            config["producer"] = serde_json::to_value(provider.producer())
                .map_err(|e| Error::Infrastructure(e.to_string()))?;
            config["policy_digest"] = serde_json::Value::String(
                snapshot.digest_for(&["retrieval.vcp.assets", "episode.synopsis"])?,
            );
            return digest(&config);
        }
        if family == "topology" {
            config["propagation"] = serde_json::to_value(resolve_wave_config(snapshot)?)
                .map_err(|e| Error::Infrastructure(e.to_string()))?;
        }
        if matches!(family, "dense" | "concept")
            && let Some(provider) = self.embedding()
        {
            config["space"] = serde_json::to_value(provider.space())
                .map_err(|e| Error::Infrastructure(e.to_string()))?;
            config["producer"] = serde_json::to_value(provider.producer())
                .map_err(|e| Error::Infrastructure(e.to_string()))?;
        }
        if family == "concept" {
            config["representation_version"] = serde_json::json!(TAG_REPRESENTATION_VERSION);
        }
        let keys: &[&str] = match family {
            "lexical" => &["serving.lexical.enabled", "episode.synopsis"],
            "dense" => &["serving.dense.enabled", "retrieval.epa", "episode.synopsis"],
            "topology" => &[
                "topology.wave.hub_beta",
                "topology.wave.hub_penalty_min",
                "topology.wave.hub_penalty_max",
                "topology.wave.outbound_budget",
                "topology.wave.max_hops",
                "topology.wave.max_states",
                "topology.wave.max_neighbors_per_node",
                "topology.wave.minimum_state_energy",
                "topology.wave.immediate_return_multiplier",
                "topology.wave.initial_budget_steps",
                "topology.wave.normal_edge_cost",
                "topology.wave.fir_gamma",
                "topology.wave.quality.host_explicit",
                "topology.wave.quality.source_evidence",
                "topology.wave.quality.cognitive_derivation",
                "topology.wave.quality.derived_structure",
                "topology.wave.quality.meaningful_use",
                "topology.wave.seed_weights.exact_target",
                "topology.wave.seed_weights.runtime_situation",
                "topology.wave.seed_weights.relation_cue",
                "topology.wave.seed_weights.entity_cue",
                "topology.wave.seed_weights.tag_cue",
                "topology.wave.seed_weights.lexical_promoted",
                "topology.wave.seed_weights.dense_promoted",
            ],
            _ => &[],
        };
        config["policy_digest"] = serde_json::Value::String(snapshot.digest_for(keys)?);
        digest(&config)
    }

    pub(crate) async fn projection_capabilities(
        &self,
        subject: SubjectId,
    ) -> Result<ProjectionCapabilities> {
        let row = sqlx::query("SELECT memory FROM subject_capabilities WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_optional(self.store.pool())
            .await
            .map_err(nous_persistence::database_error)?
            .ok_or_else(|| Error::NotFound("subject capabilities not found".into()))?;
        Ok(ProjectionCapabilities {
            memory: self.options.memory_enabled
                && row
                    .try_get("memory")
                    .map_err(nous_persistence::database_error)?,
        })
    }

    /// Prepare only the serving families required by one semantic query.
    pub async fn prepare(&self, subject: SubjectId, need: ServingNeed) -> Result<ProjectionStatus> {
        let snapshot = self.configuration.snapshot_for_subject(subject)?;
        self.prepare_with_snapshot(subject, need, &snapshot).await
    }

    pub async fn prepare_with_snapshot(
        &self,
        subject: SubjectId,
        need: ServingNeed,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> Result<ProjectionStatus> {
        let _reader = self.read_gate.clone().read_owned().await;
        self.store.require_subject(subject).await?;
        let current = self.store.serving_current(subject).await?;
        let reusable = self.store.serving_reusable(subject).await?;
        let requested = self.requested_families(need, &current);
        let mut result = ProjectionStatus::default();
        for (family, space) in requested {
            let key = if space.is_empty() {
                family.to_string()
            } else {
                format!("{family}:{space}")
            };
            let existing = current
                .iter()
                .find(|record| record.family == family && record.space == space);
            let watermark = if family == "topology" {
                sqlx::query_scalar::<_, i64>(
                    "SELECT authority_seq FROM subjects WHERE subject_id=$1",
                )
                .bind(subject.0)
                .fetch_one(self.store.pool())
                .await
                .map_err(nous_persistence::database_error)?
            } else {
                self.store
                    .projection_watermark(subject, family, &space)
                    .await?
            };
            let mut compatible = None;
            if let Some(record) = existing
                && self.compatible(record, snapshot).await
            {
                compatible = Some(record);
            }
            if compatible.is_none_or(|record| record.authority_watermark < watermark)
                && let Some(record) = self
                    .reuse_artifact(&reusable, family, &space, watermark, snapshot)
                    .await?
            {
                compatible = Some(record);
                result.reopened.push(key.clone());
            }

            if let Some(record) = compatible
                .filter(|record| record.authority_watermark >= watermark && self.loaded(record))
            {
                result.generations.insert(key, record.generation_id);
                continue;
            }
            let outcome = if let Some(record) =
                compatible.filter(|record| record.authority_watermark >= watermark)
            {
                match self.open_record(record) {
                    Ok(artifact) => {
                        self.publish_snapshot(record, artifact);
                        result.reopened.push(key.clone());
                        Ok(record.clone())
                    }
                    Err(_) => {
                        // Publication must not deduplicate a replacement against missing/corrupt current bytes.
                        self.store.fail_serving_generation(record).await?;
                        self.build_and_open(subject, family, &space, snapshot).await
                    }
                }
            } else {
                self.build_and_open(subject, family, &space, snapshot).await
            };
            if outcome.is_ok() && !result.reopened.contains(&key) {
                result.rebuilt.push(key.clone());
            }
            match outcome {
                Ok(record) => {
                    result.generations.insert(key, record.generation_id);
                }
                Err(error) => {
                    if let Some(record) = compatible
                        && let Ok(artifact) = self.open_record(record)
                    {
                        self.publish_snapshot(record, artifact);
                        result.generations.insert(key.clone(), record.generation_id);
                    }
                    result.degradation.push(Degradation {
                        code: format!("{family}_projection_unavailable"),
                        detail: Some(error.to_string()),
                    });
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn requested_families(
        &self,
        need: ServingNeed,
        current: &[ServingRecord],
    ) -> Vec<(&'static str, String)> {
        let mut requested = Vec::new();
        if need.concept || need.topology {
            let space = if need.concept_vectors {
                self.embedding()
                    .map(|p| p.space().space_hash)
                    .unwrap_or_default()
            } else {
                String::new()
            };
            requested.push(("concept", String::new()));
            if !space.is_empty() {
                requested.push(("concept", space));
            }
        }
        if need.lexical && self.options.lexical {
            requested.push(("lexical", String::new()));
        }
        if need.topology && self.options.topology {
            requested.push(("topology", String::new()));
        }
        if need.dense && self.options.dense {
            if let Some(provider) = self.embedding() {
                requested.push(("dense", provider.space().space_hash));
            } else {
                requested.extend(
                    current
                        .iter()
                        .filter(|record| record.family == "dense")
                        .map(|record| ("dense", record.space.clone())),
                );
            }
        }
        requested
    }

    async fn reuse_artifact<'a>(
        &self,
        records: &'a [ServingRecord],
        family: &str,
        space: &str,
        watermark: i64,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> Result<Option<&'a ServingRecord>> {
        for record in records {
            if record.family != family
                || record.space != space
                || record.authority_watermark < watermark
                || !self.compatible(record, snapshot).await
            {
                continue;
            }
            let artifact = match self.open_record(record) {
                Ok(artifact) => artifact,
                Err(_) => {
                    self.store.fail_serving_generation(record).await?;
                    continue;
                }
            };
            if !self.store.promote_generation(record.generation_id).await? {
                continue;
            }
            self.publish_snapshot(record, artifact);
            return Ok(Some(record));
        }
        Ok(None)
    }

    /// Administrative operation: refresh every configured family.
    pub async fn refresh(&self, subject: SubjectId) -> Result<ProjectionStatus> {
        self.prepare(
            subject,
            ServingNeed {
                lexical: self.options.lexical,
                dense: self.options.dense,
                topology: self.options.topology,
                concept: true,
                concept_vectors: self.embedding().is_some(),
            },
        )
        .await
    }

    async fn build_and_open(
        &self,
        subject: SubjectId,
        family: &str,
        space: &str,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> Result<ServingRecord> {
        let record = self.build_family(subject, family, space, snapshot).await?;
        let artifact = self.open_record(&record)?;
        self.publish_snapshot(&record, artifact);
        Ok(record)
    }

    pub(crate) async fn compatible(
        &self,
        record: &ServingRecord,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> bool {
        let identity = record.implementation_id == implementation(&record.family)
            && record.implementation_revision == env!("NOUS_SERVING_IMPLEMENTATION_DIGEST");
        identity
            && self
                .config_digest(record.subject, &record.family, snapshot)
                .await
                .ok()
                .as_deref()
                == Some(record.config_digest.as_str())
    }

    fn loaded(&self, record: &ServingRecord) -> bool {
        let snapshot = self.publisher.snapshot_for(record.subject);
        let Ok(family) = super::family::AssetFamily::parse(&record.family) else {
            return false;
        };
        match family {
            super::family::AssetFamily::Lexical => snapshot
                .lexical
                .as_ref()
                .is_some_and(|index| index.generation_id == record.generation_id),
            super::family::AssetFamily::Topology => {
                snapshot
                    .topology
                    .as_ref()
                    .is_some_and(|graph| graph.generation_id == record.generation_id)
                    || snapshot
                        .vcp
                        .as_ref()
                        .is_some_and(|assets| assets.generation_id == record.generation_id)
            }
            super::family::AssetFamily::Dense => snapshot
                .dense
                .iter()
                .any(|index| index.generation_id == record.generation_id),
            super::family::AssetFamily::Concept => snapshot
                .concept
                .iter()
                .any(|generation| generation.generation_id == record.generation_id),
        }
    }

    pub(crate) fn open_record(&self, record: &ServingRecord) -> Result<OpenArtifact> {
        let path = Path::new(&record.artifact_location);
        let sums = checksums(path)?;
        if digest(&sums)? != record.artifact_hash {
            return Err(Error::Infrastructure(
                "serving generation checksum mismatch".into(),
            ));
        }
        self.readback(path, &record.family, record.generation_id)
    }

    pub(crate) fn readback(
        &self,
        path: &Path,
        family: &str,
        id: ServingGenerationId,
    ) -> Result<OpenArtifact> {
        match super::family::AssetFamily::parse(family)? {
            super::family::AssetFamily::Lexical => {
                let mut index = LexicalGeneration::open(path)?;
                index.generation_id = id;
                Ok(OpenArtifact::Lexical(Arc::new(index)))
            }
            super::family::AssetFamily::Concept => {
                let generation: ConceptGeneration = read_json(&path.join("concept.json"))?;
                generation.validate()?;
                if generation.generation_id != id {
                    return Err(Error::Infrastructure(
                        "concept generation identity mismatch".into(),
                    ));
                }
                Ok(OpenArtifact::Concept(Arc::new(generation)))
            }
            super::family::AssetFamily::Topology => {
                if path.join("vcp.json").exists() {
                    let assets: VcpGeneration = read_json(&path.join("vcp.json"))?;
                    if assets.generation_id != id {
                        return Err(Error::Infrastructure(
                            "VCP generation identity mismatch".into(),
                        ));
                    }
                    assets.validate()?;
                    return Ok(OpenArtifact::Vcp(Arc::new(VcpServingGeneration::open(
                        assets, path,
                    )?)));
                }
                let artifact: TopologyArtifact = read_json(&path.join("topology.json"))?;
                if artifact.generation_id != id {
                    return Err(Error::Infrastructure(
                        "topology generation identity mismatch".into(),
                    ));
                }
                Ok(OpenArtifact::Topology(Arc::new(
                    WaveGraphGeneration::from_artifact(artifact)?,
                )))
            }
            super::family::AssetFamily::Dense => {
                let manifest: DenseManifest = read_json(&path.join("records.json"))?;
                if let Some(basis) = &manifest.basis
                    && (basis.generation_id != id
                        || basis.basis.embedding_space != manifest.space.space_hash)
                {
                    return Err(Error::Infrastructure(
                        "cue basis generation/space mismatch".into(),
                    ));
                }
                let dense = DenseGeneration::open(
                    &path.join("vectors.usearch"),
                    id,
                    manifest.space,
                    manifest.records,
                )?;
                Ok(OpenArtifact::Dense(
                    Arc::new(dense),
                    manifest.basis.map(Arc::new),
                ))
            }
        }
    }

    fn publish_snapshot(&self, record: &ServingRecord, artifact: OpenArtifact) {
        self.publisher.update_for(record.subject, |snapshot| {
            snapshot.install(artifact.clone());
        });
    }
}
