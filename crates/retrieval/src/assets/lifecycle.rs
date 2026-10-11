// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{assets::build::DenseManifest, assets::files::*, *};
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
        let family_keys = super::family::family_config_keys(family).ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "unknown physical Serving family",
            )
        })?;
        let is_vcp_topology = family == "topology"
            && matches!(
                snapshot.get(nous_runtime::COGNITIVE_PROFILE)?,
                nous_runtime::CognitiveProfile::VcpDtsc
                    | nous_runtime::CognitiveProfile::VcpRiverMemo
            );
        let mut config_keys = family_keys;
        if family == "topology" {
            if is_vcp_topology {
                config_keys.retain(|key| !crate::WAVE_BUILD_CONFIG_KEYS.contains(key));
            } else {
                config_keys.retain(|key| *key != crate::VCP_ASSETS.path());
                // Native Wave topology consumes only typed topology nodes and
                // edges. Episode excerpts are read alongside those rows for
                // other families, but they do not enter this graph artifact.
                config_keys.retain(|key| *key != crate::EPISODE_SYNOPSIS.path());
            }
        }
        let configuration_digest = snapshot.digest_for(&config_keys)?;
        let capabilities = self.projection_capabilities(subject).await?;
        let mut config = serde_json::json!({});
        config["configuration_digest"] = serde_json::json!(configuration_digest);
        if matches!(family, "lexical" | "dense" | "topology") {
            config["memory_enabled"] = serde_json::json!(capabilities.memory);
        }
        if is_vcp_topology {
            let provider = self.embedding().ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::Unavailable,
                    "Service unavailable",
                )
            })?;
            config["space"] = serde_json::to_value(provider.space()).map_err(|e| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not serialize the VCP embedding space configuration",
                )
                .with_source(e)
            })?;
            config["producer"] = serde_json::to_value(provider.producer()).map_err(|e| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not serialize the VCP producer configuration",
                )
                .with_source(e)
            })?;
            config["build_policy"] = serde_json::to_value(snapshot.get(crate::VCP_ASSETS)?)
                .map_err(|e| {
                    Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not serialize the VCP asset build policy",
                    )
                    .with_source(e)
                })?;
            config["synopsis_budget"] = serde_json::json!(snapshot.get(crate::EPISODE_SYNOPSIS)?);
            return digest(&config);
        }
        if family == "topology" {
            config["build_policy"] = serde_json::to_value(resolve_wave_build_policy(snapshot)?)
                .map_err(|e| {
                    Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not serialize the Wave topology build policy",
                    )
                    .with_source(e)
                })?;
        }
        if matches!(family, "dense" | "concept")
            && let Some(provider) = self.embedding()
        {
            config["space"] = serde_json::to_value(provider.space()).map_err(|e| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not serialize the Serving embedding space configuration",
                )
                .with_source(e)
            })?;
            config["producer"] = serde_json::to_value(provider.producer()).map_err(|e| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not serialize the Serving producer configuration",
                )
                .with_source(e)
            })?;
        }
        match family {
            "lexical" => {
                config["synopsis_budget"] =
                    serde_json::json!(snapshot.get(crate::EPISODE_SYNOPSIS)?);
            }
            "dense" => {}
            "concept" => {
                config["representation_version"] = serde_json::json!(TAG_REPRESENTATION_VERSION);
            }
            "topology" => {}
            _ => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "unknown physical Serving family",
                ));
            }
        }
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
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::NotFound,
                    "subject capabilities not found",
                )
            })?;
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
        let (record, artifact) = self.build_family(subject, family, space, snapshot).await?;
        self.publish_snapshot(&record, artifact);
        Ok(record)
    }

    pub(crate) async fn compatible(
        &self,
        record: &ServingRecord,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> bool {
        let Ok(family) = super::family::AssetFamily::parse(&record.family) else {
            return false;
        };
        let profile = if matches!(family, super::family::AssetFamily::Topology) {
            record.metadata.get("cognitive_profile").and_then(|value| {
                serde_json::from_value::<nous_runtime::CognitiveProfile>(value.clone()).ok()
            })
        } else {
            None
        };
        let identity = record.implementation_id == family.implementation()
            && record.implementation_revision == family.implementation_revision(profile);
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
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Serving artifact checksum does not match its recorded hash",
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
                    return Err(Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Concept Serving artifact has an unexpected generation ID",
                    ));
                }
                Ok(OpenArtifact::Concept(Arc::new(generation)))
            }
            super::family::AssetFamily::Topology => {
                if path.join("vcp.json").exists() {
                    let assets: VcpGeneration = read_json(&path.join("vcp.json"))?;
                    if assets.generation_id != id {
                        return Err(Error::new(
                            nous_core::ErrorOwner::Resource,
                            nous_core::ErrorCode::InternalFailure,
                            "VCP topology artifact has an unexpected generation ID",
                        ));
                    }
                    assets.validate()?;
                    return Ok(OpenArtifact::Vcp(Arc::new(VcpServingGeneration::open(
                        assets, path,
                    )?)));
                }
                let artifact: TopologyArtifact = read_json(&path.join("topology.json"))?;
                if artifact.generation_id != id {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Topology Serving artifact has an unexpected generation ID",
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
                    return Err(Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Dense Serving manifest does not match its vector generation",
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
