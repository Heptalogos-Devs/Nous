use crate::{
    ReadView,
    authority::records,
    cognition::{memory, structured},
    model::calls,
    storage_error,
};
use nous_domain::{media::*, *};
use sqlx::Row;
use std::collections::BTreeSet;

impl ReadView {
    /// All selected bytes and producer histories are checked against the same source-read view.
    pub async fn subject_package(
        &mut self,
        ctx: &CallContext,
        request: &ExportBundleRequest,
        created_at: String,
    ) -> Result<SubjectPackage> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        let (ns, sub) = (&request.namespace_id, &request.subject_id);
        let subject = records::subject(&mut self.tx, ns, sub)
            .await?
            .ok_or_else(crate::not_found)?;
        let mut package = SubjectPackage {
            package_id: request.package_id.clone(),
            name: request.name.clone(),
            created_at,
            watermark: self.watermark,
            subject,
            materials: vec![],
            observations: vec![],
            exposures: vec![],
            memories: vec![],
            cognition: vec![],
            source_regions: vec![],
            representations: vec![],
            derived_regions: vec![],
            calls: vec![],
            purges: vec![],
            tombstones: vec![],
            imports: vec![],
            vectors: BundleVectors::default(),
        };
        let mut material_ids = BTreeSet::new();
        let rows=sqlx::query("SELECT * FROM observations WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,id").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let observation = records::observation_row(row)?;
            self.require_source_read(
                ctx,
                ns,
                sub,
                &RetrievalTarget::Media(MediaReference::Original {
                    material_id: observation.material_id.clone(),
                }),
            )
            .await?;
            material_ids.insert(observation.material_id.clone());
            package.observations.push(observation);
        }
        let rows=sqlx::query("SELECT body FROM media_exposures WHERE namespace_id=? AND subject_id=? ORDER BY observation_id,locator").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let exposure: MediaExposure = serde_json::from_str(row.get("body"))?;
            self.require_source_read(
                ctx,
                ns,
                sub,
                &RetrievalTarget::Media(exposure.locator.clone()),
            )
            .await?;
            package.exposures.push(exposure);
        }
        for id in &material_ids {
            package.materials.push(MediaOriginal {
                material: records::material(&mut self.tx, ns, id)
                    .await?
                    .ok_or_else(crate::not_found)?,
                details: self.material_details(ns, id).await?,
                details_redacted: false,
            });
        }
        let rows=sqlx::query("SELECT * FROM memory_revisions WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,memory_id,revision").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let memory = memory::memory_row(row)?;
            self.require_source_read(
                ctx,
                ns,
                sub,
                &RetrievalTarget::Cognition(CognitiveRef {
                    kind: CognitiveKind::Memory,
                    id: memory.id.to_string(),
                    revision: memory.revision,
                }),
            )
            .await?;
            package.memories.push(memory);
        }
        let rows=sqlx::query("SELECT object_id,revision FROM structured_revisions WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,object_id,revision").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let id = CognitiveId(row.get("object_id"));
            let revision = row.get("revision");
            let object = structured::read::object(&mut self.tx, ns, &id, Some(revision)).await?;
            self.require_source_read(
                ctx,
                ns,
                sub,
                &RetrievalTarget::Cognition(object.reference()),
            )
            .await?;
            package.cognition.push(object);
        }
        let rows = sqlx::query(
            "SELECT body FROM source_regions WHERE namespace_id=? ORDER BY commit_seq,id",
        )
        .bind(ns.as_str())
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        for row in rows {
            let value: SourceRegion = serde_json::from_str(row.get("body"))?;
            if material_ids.contains(&value.material_id) {
                self.require_source_read(
                    ctx,
                    ns,
                    sub,
                    &RetrievalTarget::Media(MediaReference::SourceRegion {
                        id: value.id.clone(),
                    }),
                )
                .await?;
                package.source_regions.push(value);
            }
        }
        let rows=sqlx::query("SELECT body FROM media_representations WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,id,revision").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let value: DerivedRepresentation = serde_json::from_str(row.get("body"))?;
            self.require_source_read(
                ctx,
                ns,
                sub,
                &RetrievalTarget::Media(MediaReference::Representation {
                    reference: value.reference(),
                }),
            )
            .await?;
            package.representations.push(value);
        }
        let rows=sqlx::query("SELECT body FROM derived_regions WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,id").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let value: DerivedRegion = serde_json::from_str(row.get("body"))?;
            self.require_source_read(
                ctx,
                ns,
                sub,
                &RetrievalTarget::Media(MediaReference::DerivedRegion {
                    id: value.id.clone(),
                }),
            )
            .await?;
            package.derived_regions.push(value);
        }
        package.vectors = self.bundle_vectors(ctx, ns, sub).await?;
        let ids = package
            .memories
            .iter()
            .filter_map(|v| v.model_call_id.clone())
            .chain(
                package
                    .cognition
                    .iter()
                    .filter_map(|v| v.producer_call_id.clone()),
            )
            .chain(
                package
                    .representations
                    .iter()
                    .filter_map(|v| v.producer.model_call_id.clone()),
            )
            .chain(
                package
                    .vectors
                    .contents
                    .iter()
                    .map(|v| v.model_call_id.clone()),
            )
            .collect::<BTreeSet<_>>();
        for id in ids {
            let call = calls::model_call(&mut self.tx, &id).await?;
            if (!ctx.source_read.is_empty()
                || matches!(&call.origin, ModelCallOrigin::Imported { .. }))
                && call.request.get("redacted") != Some(&serde_json::Value::Bool(true))
            {
                let sources: Vec<SourceReadRef> = serde_json::from_value(
                    call.request
                        .pointer("/preview/sources")
                        .cloned()
                        .ok_or_else(|| {
                            Error::new(
                                ErrorCode::Forbidden,
                                "exported model history has no attributable source catalog",
                            )
                        })?,
                )
                .map_err(|_| {
                    Error::new(
                        ErrorCode::Forbidden,
                        "exported model history source catalog is unavailable",
                    )
                })?;
                let mapped = self.imported_model_sources(&call, &sources).await?;
                self.require_source_requirements(ctx, ns, sub, &mapped)
                    .await?;
            }
            package.calls.push(call);
        }
        let rows = sqlx::query(
            "SELECT * FROM memory_purges WHERE namespace_id=? AND subject_id=? ORDER BY memory_id",
        )
        .bind(ns.as_str())
        .bind(sub.as_str())
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        for row in rows {
            package
                .purges
                .push(crate::cognition::management::purge_row(row)?);
        }
        let rows=sqlx::query("SELECT body FROM source_tombstones WHERE namespace_id=? AND subject_id=? ORDER BY target_key").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let tombstone: SourceTombstone = serde_json::from_str(row.get("body"))?;
            for source in &tombstone.sources {
                if let Some(target) = source.retrieval_target() {
                    self.require_source_read(ctx, ns, sub, &target).await?;
                }
            }
            package.tombstones.push(tombstone);
        }
        let rows = sqlx::query(
            "SELECT id FROM bundle_installations WHERE namespace_id=? AND subject_id=? ORDER BY id",
        )
        .bind(ns.as_str())
        .bind(sub.as_str())
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        for row in rows {
            let id: String = row.get("id");
            let installation = self.bundle_installation(&id).await?;
            let resources=sqlx::query("SELECT provenance FROM bundle_resources WHERE installation_id=? ORDER BY kind,original_id,revision").bind(&id).fetch_all(&mut *self.tx).await.map_err(storage_error)?.into_iter().map(|r|serde_json::from_str(r.get("provenance")).map_err(Error::from)).collect::<Result<Vec<_>>>()?;
            if !package.imports.iter().any(|v| v.installation.id == id) {
                package.imports.push(BundleImportProvenance {
                    installation,
                    resources,
                    vector_dispositions: self.bundle_vector_dispositions(&id).await?,
                });
            }
            for source in self.bundle_ancestry(&id).await? {
                if !package
                    .imports
                    .iter()
                    .any(|v| v.installation.id == source.installation.id)
                {
                    package.imports.push(source);
                }
            }
        }
        package.validate()?;
        Ok(package)
    }
}
