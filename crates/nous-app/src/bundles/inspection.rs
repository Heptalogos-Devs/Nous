use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectBundleRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub material_id: MaterialId,
}
#[derive(Debug, Clone, Serialize)]
pub struct BundleInspectionSummary {
    pub format: String,
    pub package: PackageDescriptor,
    pub content_level: BundleContentLevel,
    pub counts: BTreeMap<String, u64>,
    pub manifest: Value,
    pub material: Material,
    pub vector_spaces: Vec<EmbeddingSpace>,
    pub vector_members: Vec<String>,
    pub image_assets: u64,
}

impl App {
    pub async fn inspect_bundle(
        &self,
        ctx: &CallContext,
        request: InspectBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<BundleInspectionSummary> {
        let owner = self.clone();
        let ctx = ctx.clone();
        execution::owned_async(cancel, move |local| async move {
            owner.inspect_bundle_inner(&ctx, request, &local).await
        })
        .await
    }
    async fn inspect_bundle_inner(
        &self,
        ctx: &CallContext,
        request: InspectBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<BundleInspectionSummary> {
        let input = self
            .bundle_input(
                ctx,
                &request.namespace_id,
                &request.subject_id,
                &request.material_id,
                cancel,
            )
            .await?;
        let path = input.path.clone();
        let limits = input.policy.limits.clone();
        let material = input.original.material.clone();
        execution::blocking(cancel, move |_| {
            if let Some(loaded) = archive::load(&path, &limits)? {
                let p = loaded.package;
                let counts = package_counts(&p);
                let image_assets = p
                    .materials
                    .iter()
                    .filter(|m| matches!(m.material.metadata, MediaMetadata::Image { .. }))
                    .count() as u64;
                let manifest = loaded.manifest;
                Ok(BundleInspectionSummary {
                    format: archive::FORMAT.into(),
                    package: p.package,
                    content_level: p.content_level,
                    counts,
                    manifest,
                    material,
                    vector_spaces: p.vectors.spaces,
                    vector_members: vec![],
                    image_assets,
                })
            } else {
                let parsed = crate::bundle_format::inspect(std::fs::File::open(&path)?, &limits)
                    .map_err(archive::format_error)?;
                let package = PackageDescriptor {
                    id: parsed.manifest.package.id.clone(),
                    name: parsed.manifest.package.name.clone(),
                    version: Some(parsed.manifest.package.version.clone()),
                    description: parsed
                        .manifest
                        .package
                        .extra
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    author: parsed
                        .manifest
                        .package
                        .extra
                        .get("author")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                };
                let content_level = match parsed.manifest.content_level {
                    crate::bundle_format::ContentLevel::Knowledge => BundleContentLevel::Knowledge,
                    crate::bundle_format::ContentLevel::Full => BundleContentLevel::Full,
                };
                let image_assets = parsed
                    .images
                    .as_ref()
                    .map_or(0, |images| images.assets.len() as u64);
                Ok(BundleInspectionSummary {
                    format: parsed.manifest.format.clone(),
                    package,
                    content_level,
                    counts: parsed.manifest.counts.clone(),
                    manifest: serde_json::to_value(&parsed.manifest)?,
                    material,
                    vector_spaces: vec![],
                    vector_members: parsed.index.vector_members.keys().cloned().collect(),
                    image_assets,
                })
            }
        })
        .await
    }
}
pub(super) fn package_counts(package: &SubjectPackage) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::from([
        (
            "memories".into(),
            package
                .memories
                .iter()
                .map(|m| &m.id)
                .collect::<BTreeSet<_>>()
                .len() as u64,
        ),
        ("memory_revisions".into(), package.memories.len() as u64),
        ("materials".into(), package.materials.len() as u64),
        ("observations".into(), package.observations.len() as u64),
        (
            "vector_contents".into(),
            package.vectors.contents.len() as u64,
        ),
        (
            "vector_associations".into(),
            package.vectors.associations.len() as u64,
        ),
    ]);
    for value in &package.cognition {
        let kind = serde_json::to_value(value.kind())
            .expect("kind serializable")
            .as_str()
            .expect("kind string")
            .to_owned();
        *counts.entry(format!("{kind}_revisions")).or_default() += 1;
    }
    counts
}
