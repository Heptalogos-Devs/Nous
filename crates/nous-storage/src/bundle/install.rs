use super::remap::Remap;
use crate::{WriteTransaction, storage_error};
use nous_domain::{media::*, *};
use std::collections::VecDeque;

enum Pending {
    Source(SourceRegion),
    Representation(DerivedRepresentation),
    Region(DerivedRegion),
    Memory(Memory),
    Cognition(StructuredObject),
}
impl WriteTransaction {
    pub async fn install_subject_package(
        &mut self,
        package: &SubjectPackage,
        request: &InstallBundleRequest,
        digest: &str,
    ) -> Result<BundleInstallation> {
        package.validate()?;
        self.context()
            .require_subject(&request.namespace_id, &request.subject_id)?;
        self.subject(&request.namespace_id, &request.subject_id)
            .await?;
        let archive = self
            .material(&request.namespace_id, &request.material_id)
            .await?;
        if archive.content_hash.is_empty() {
            return Err(Error::invalid("bundle archive has no digest"));
        }
        if let Some(existing) = self
            .bundle_by_package(
                &request.namespace_id,
                &request.subject_id,
                &package.package_id,
            )
            .await?
        {
            if existing.content_digest != digest {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "package ID is already registered with different content",
                ));
            }
            if existing.uninstalled_at.is_none() {
                return Ok(existing);
            }
        }
        if request.mode == BundleInstallMode::Restore {
            let count:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM observations WHERE namespace_id=? AND subject_id=?)+(SELECT count(*) FROM memories WHERE namespace_id=? AND subject_id=?)+(SELECT count(*) FROM structured_objects WHERE namespace_id=? AND subject_id=?)")
                .bind(request.namespace_id.as_str()).bind(request.subject_id.as_str()).bind(request.namespace_id.as_str()).bind(request.subject_id.as_str()).bind(request.namespace_id.as_str()).bind(request.subject_id.as_str()).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
            if count != 0 {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "subject restore requires an empty target subject",
                ));
            }
        }
        let installation = BundleInstallation {
            id: MaterialId::new().to_string(),
            namespace_id: request.namespace_id.clone(),
            subject_id: request.subject_id.clone(),
            source_namespace_id: package.subject.namespace_id.clone(),
            source_subject_id: package.subject.id.clone(),
            package_id: package.package_id.clone(),
            name: package.name.clone(),
            content_digest: digest.into(),
            archive_material_id: request.material_id.clone(),
            installed_at: self.time().recorded(),
            uninstalled_at: None,
            commit_seq: self.commit_seq(),
            resource_counts: Default::default(),
        };
        self.register_bundle(&installation).await?;
        self.inherit_bundle_provenance(&installation.id, &package.imports)
            .await?;
        let map = Remap::new(
            package,
            &request.namespace_id,
            &request.subject_id,
            self.commit_seq(),
            self.time().recorded(),
        );
        for source in &package.calls {
            let mut call = source.clone();
            call.id = ModelCallId(map.id(source.id.as_str())?);
            call.namespace_id = request.namespace_id.clone();
            call.subject_id = request.subject_id.clone();
            let (original_call_id, original_namespace_id, original_subject_id) =
                match &source.origin {
                    ModelCallOrigin::Local => (
                        source.id.clone(),
                        source.namespace_id.clone(),
                        source.subject_id.clone(),
                    ),
                    ModelCallOrigin::Imported {
                        original_call_id,
                        original_namespace_id,
                        original_subject_id,
                        ..
                    } => (
                        original_call_id.clone(),
                        original_namespace_id.clone(),
                        original_subject_id.clone(),
                    ),
                };
            call.origin = ModelCallOrigin::Imported {
                installation_id: installation.id.clone(),
                original_call_id,
                original_namespace_id,
                original_subject_id,
            };
            self.insert_call_record(&call, None).await?;
            self.resource(
                &installation,
                "model_call",
                source.id.as_str(),
                call.id.as_str(),
                None,
                &source.created_at,
                0,
                None,
            )
            .await?;
        }
        for source in &package.materials {
            let value = map.material(source)?;
            self.insert_material(&value.material).await?;
            self.set_material_details(&request.namespace_id, &value.material.id, &value.details)
                .await?;
            self.resource(
                &installation,
                "material",
                source.material.id.as_str(),
                value.material.id.as_str(),
                None,
                &source.material.recorded_at,
                source.material.commit_seq,
                None,
            )
            .await?;
        }
        for source in &package.observations {
            let value = map.observation(source)?;
            self.insert_observation(&value).await?;
            self.resource(
                &installation,
                "observation",
                source.id.as_str(),
                value.id.as_str(),
                None,
                &source.recorded_at,
                source.commit_seq,
                Some(source.clone()),
            )
            .await?;
        }
        self.record_bundle_exposures(&installation.id, &package.exposures)
            .await?;
        for source in &package.purges {
            let id = map.id(source.id.as_str())?;
            sqlx::query("INSERT INTO memory_purges(memory_id,namespace_id,subject_id,revision,commit_seq) VALUES(?,?,?,?,?)").bind(id).bind(request.namespace_id.as_str()).bind(request.subject_id.as_str()).bind(source.revision).bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(crate::write_error)?;
        }
        for source in &package.tombstones {
            self.insert_source_tombstone(&map.tombstone(source)?)
                .await?;
        }
        let mut pending = VecDeque::new();
        for source in &package.source_regions {
            let value: SourceRegion = map.region(source)?;
            self.resource(
                &installation,
                "source_region",
                source.id.as_str(),
                value.id.as_str(),
                None,
                &source.recorded_at,
                source.commit_seq,
                None,
            )
            .await?;
            pending.push_back(Pending::Source(value));
        }
        for source in &package.representations {
            let value = map.representation(source)?;
            self.resource(
                &installation,
                "representation",
                source.id.as_str(),
                value.id.as_str(),
                Some(value.revision),
                &source.recorded_at,
                source.commit_seq,
                None,
            )
            .await?;
            pending.push_back(Pending::Representation(value));
        }
        for source in &package.derived_regions {
            let value: DerivedRegion = map.region(source)?;
            self.resource(
                &installation,
                "derived_region",
                source.id.as_str(),
                value.id.as_str(),
                None,
                &source.recorded_at,
                source.commit_seq,
                None,
            )
            .await?;
            pending.push_back(Pending::Region(value));
        }
        for source in &package.memories {
            let value = map.memory(source)?;
            self.resource(
                &installation,
                "memory",
                source.id.as_str(),
                value.id.as_str(),
                Some(value.revision),
                &source.recorded_at,
                source.commit_seq,
                None,
            )
            .await?;
            pending.push_back(Pending::Memory(value));
        }
        for source in &package.cognition {
            let value = map.cognition(source)?;
            self.resource(
                &installation,
                "cognition",
                source.id.as_str(),
                value.id.as_str(),
                Some(value.revision),
                &source.recorded_at,
                source.commit_seq,
                None,
            )
            .await?;
            pending.push_back(Pending::Cognition(value));
        }
        // Existing owner validation resolves exact dependencies. Savepoints keep a failed
        // deferred item from altering the candidate graph while later inputs are admitted.
        while !pending.is_empty() {
            let size = pending.len();
            let mut progress = false;
            let mut first_error = None;
            for _ in 0..size {
                let value = pending.pop_front().expect("pending length");
                sqlx::query("SAVEPOINT bundle_item")
                    .execute(&mut *self.tx)
                    .await
                    .map_err(storage_error)?;
                let result = match &value {
                    Pending::Source(v) => {
                        self.put_source_region(v.clone()).await.and_then(|saved| {
                            if saved.id == v.id {
                                Ok(())
                            } else {
                                Err(Error::new(
                                    ErrorCode::Conflict,
                                    "duplicate source locator in package",
                                ))
                            }
                        })
                    }
                    Pending::Representation(v) => self.save_representation(v, v.revision - 1).await,
                    Pending::Region(v) => {
                        self.put_derived_region(v.clone()).await.and_then(|saved| {
                            if saved.id == v.id {
                                Ok(())
                            } else {
                                Err(Error::new(
                                    ErrorCode::Conflict,
                                    "duplicate derived locator in package",
                                ))
                            }
                        })
                    }
                    Pending::Memory(v) => {
                        self.save_memory(v, (v.revision > 1).then_some(v.revision - 1))
                            .await
                    }
                    Pending::Cognition(v) => {
                        self.save_structured(v, (v.revision > 1).then_some(v.revision - 1))
                            .await
                    }
                };
                match result {
                    Ok(()) => progress = true,
                    Err(error) => {
                        sqlx::query("ROLLBACK TO bundle_item")
                            .execute(&mut *self.tx)
                            .await
                            .map_err(storage_error)?;
                        if !matches!(error.code, ErrorCode::NotFound | ErrorCode::Conflict) {
                            return Err(error);
                        }
                        first_error.get_or_insert(error);
                        pending.push_back(value);
                    }
                }
                sqlx::query("RELEASE bundle_item")
                    .execute(&mut *self.tx)
                    .await
                    .map_err(storage_error)?;
            }
            if !progress {
                return Err(first_error.unwrap_or_else(|| {
                    Error::invalid("package dependency graph cannot be installed")
                }));
            }
        }
        for source in &package.tombstones {
            let value = map.tombstone(source)?;
            for child in value.sources {
                crate::source::references::validate_source(
                    &mut self.tx,
                    &request.namespace_id,
                    &request.subject_id,
                    &child,
                )
                .await?;
            }
        }
        self.install_bundle_vectors(package, &installation, &map)
            .await?;
        self.bundle_installation(&installation.id).await
    }
}
