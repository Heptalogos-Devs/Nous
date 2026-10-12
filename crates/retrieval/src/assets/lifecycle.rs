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
        space: &str,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> Result<String> {
        let capabilities = self.projection_capabilities(subject).await?;
        let mut config = serde_json::json!({"memory_enabled":capabilities.memory});
        if family == "topology" && space.starts_with("vcp:") {
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
            "topology" => &[],
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
        Ok(self.prepare_assets(subject, need, snapshot, None).await?.0)
    }

    /// Resolve and open each required family once, assembling the request view
    /// directly. Only current assets are published to the process cache.
    pub(crate) async fn prepare_assets(
        &self,
        subject: SubjectId,
        need: ServingNeed,
        config: &nous_configuration::ConfigSnapshot,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<(ProjectionStatus, ServingSnapshot)> {
        self.store.require_subject(subject).await?;
        let current = if view.is_none() {
            self.store.serving_current(subject).await?
        } else {
            Vec::new()
        };
        let reusable = match view {
            Some(view) => {
                self.store
                    .historical_serving_reusable(subject, &view.snapshot_digest)
                    .await?
            }
            None => self.store.serving_reusable(subject).await?,
        };
        let current_ids = current
            .iter()
            .map(|record| record.generation_id)
            .collect::<std::collections::HashSet<_>>();
        let cached = self.publisher.snapshot_for(subject);
        let mut serving = ServingSnapshot {
            view_digest: view.map(|view| view.snapshot_digest.clone()),
            ..Default::default()
        };
        let mut status = ProjectionStatus::default();
        let watermarks = if view.is_none() {
            self.store.projection_watermarks(subject).await?
        } else {
            Default::default()
        };
        for (family, space) in self.requested_families(
            need,
            if view.is_some() { &reusable } else { &current },
            config,
        )? {
            let key = if space.is_empty() {
                family.to_owned()
            } else {
                format!("{family}:{space}")
            };
            let watermark = watermarks.desired(family, &space);
            let mut selected = None;
            let mut fallback = None;
            let mut checked = std::collections::HashSet::new();
            let config_digest = self
                .config_digest(subject, family, &space, config)
                .await
                .ok();
            for record in current
                .iter()
                .chain(reusable.iter())
                .filter(|record| record.family == family && record.space == space)
            {
                if !checked.insert(record.generation_id)
                    || config_digest.as_deref() != Some(record.config_digest.as_str())
                {
                    continue;
                }
                let artifact = if view.is_none() {
                    cached
                        .artifact(record.generation_id)
                        .map(Ok)
                        .unwrap_or_else(|| self.open_record(record))
                } else {
                    self.open_record(record)
                };
                let artifact = match artifact {
                    Ok(artifact) => artifact,
                    Err(_) => {
                        self.store.fail_serving_generation(record).await?;
                        continue;
                    }
                };
                if record.authority_watermark < watermark {
                    // A stale current projection remains an explicit degraded
                    // fallback if rebuilding fails; never use it for history.
                    fallback = current_ids
                        .contains(&record.generation_id)
                        .then(|| (record.clone(), artifact));
                    continue;
                }
                let loaded = view.is_none() && cached.contains_generation(record.generation_id);
                if view.is_none()
                    && !current_ids.contains(&record.generation_id)
                    && !self.store.promote_generation(record.generation_id).await?
                {
                    continue;
                }
                if !loaded {
                    status.reopened.push(key.clone());
                }
                selected = Some((record.clone(), artifact));
                break;
            }
            if selected.is_none() {
                match self
                    .build_family_in_view(subject, family, &space, config, view, &serving)
                    .await
                {
                    Ok(asset) => {
                        status.rebuilt.push(key.clone());
                        selected = Some(asset);
                    }
                    Err(error) => status.degradation.push(Degradation {
                        code: format!("{family}_projection_unavailable"),
                        detail: Some(error.to_string()),
                    }),
                }
            }
            if let Some((record, artifact)) = selected.or(fallback) {
                if view.is_none() && !cached.contains_generation(record.generation_id) {
                    self.publish_snapshot(&record, artifact.clone());
                }
                serving.install(artifact);
                status.generations.insert(key, record.generation_id);
            }
        }
        Ok((status, serving))
    }

    pub(crate) fn requested_families(
        &self,
        need: ServingNeed,
        current: &[ServingRecord],
        config: &nous_configuration::ConfigSnapshot,
    ) -> Result<Vec<(&'static str, String)>> {
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
            let space = if matches!(
                config.get(nous_runtime::COGNITIVE_PROFILE)?,
                nous_runtime::CognitiveProfile::VcpDtsc
                    | nous_runtime::CognitiveProfile::VcpRiverMemo
            ) {
                let provider = self.embedding().ok_or_else(|| {
                    Error::Unavailable(
                        "VCP serving build requires configured embedding material".into(),
                    )
                })?;
                format!("vcp:{}", provider.space().space_hash)
            } else {
                String::new()
            };
            requested.push(("topology", space));
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
        Ok(requested)
    }

    /// Administrative operation: refresh every configured family.
    pub async fn refresh(&self, subject: SubjectId) -> Result<ProjectionStatus> {
        let status = self
            .prepare(
                subject,
                ServingNeed {
                    lexical: self.options.lexical,
                    dense: self.options.dense,
                    topology: self.options.topology,
                    concept: true,
                    concept_vectors: self.embedding().is_some(),
                },
            )
            .await?;
        let config = self.configuration.snapshot_for_subject(subject)?;
        self.reclaim_retired(
            subject,
            std::time::Duration::from_secs(config.get(RETIRED_GRACE_SECONDS)?),
        )
        .await?;
        Ok(status)
    }

    pub(crate) fn open_record(&self, record: &ServingRecord) -> Result<OpenArtifact> {
        let path = self.artifact_directory(record.generation_id);
        let sums = checksums(&path)?;
        if digest(&sums)? != record.artifact_hash {
            return Err(Error::Infrastructure(
                "serving generation checksum mismatch".into(),
            ));
        }
        self.readback(&path, &record.family, record.generation_id)
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
