use crate::{
    ReadView,
    authority::records,
    cognition::{memory, structured},
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
        request.validate()?;
        let (ns, sub) = (&request.namespace_id, &request.subject_id);
        let subject = records::subject(&mut self.tx, ns, sub)
            .await?
            .ok_or_else(crate::not_found)?;
        let mut package = SubjectPackage {
            addresses: vec![],
            package: request.package.clone(),
            content_level: request.content_level,
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
            material_ids.insert(observation.material_id.clone());
            package.observations.push(observation);
        }
        let rows=sqlx::query("SELECT body FROM media_exposures WHERE namespace_id=? AND subject_id=? ORDER BY observation_id,locator").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let exposure: MediaExposure = serde_json::from_str(row.get("body"))?;
            package.exposures.push(exposure);
        }
        material_ids.extend(self.subject_material_ids(ns, sub).await?);
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
            package.memories.push(memory);
        }
        let rows=sqlx::query("SELECT object_id,revision FROM structured_revisions WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,object_id,revision").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let id = CognitiveId(row.get("object_id"));
            let revision = row.get("revision");
            let object = structured::read::object(&mut self.tx, ns, &id, Some(revision)).await?;
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
                package.source_regions.push(value);
            }
        }
        let rows=sqlx::query("SELECT body FROM media_representations WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,id,revision").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let value: DerivedRepresentation = serde_json::from_str(row.get("body"))?;
            package.representations.push(value);
        }
        let rows=sqlx::query("SELECT body FROM derived_regions WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,id").bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let value: DerivedRegion = serde_json::from_str(row.get("body"))?;
            package.derived_regions.push(value);
        }
        package.vectors = self.bundle_vector_snapshot(ns, sub).await?;
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
        self.select_bundle_snapshot(ctx, request, &mut package)
            .await?;
        self.authorize_bundle_snapshot(ctx, &package).await?;
        package.addresses = self.package_reference_addresses(&package).await?;
        package.validate()?;
        Ok(package)
    }
}
