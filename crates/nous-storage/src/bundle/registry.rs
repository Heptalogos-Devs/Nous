use crate::{Database, ReadView, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::{media::*, *};
use serde::Serialize;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct BundleWithdrawal {
    pub installation: BundleInstallation,
    pub withdrawn: Vec<RetrievalTarget>,
    pub retained_modified: Vec<String>,
    pub already_retired: Vec<String>,
}
impl Database {
    pub async fn bundle_vector_dispositions(
        &self,
        id: &str,
    ) -> Result<Vec<BundleVectorDisposition>> {
        let mut view = self.read_view().await?;
        let result = view.bundle_vector_dispositions(id).await?;
        view.finish().await?;
        Ok(result)
    }
    pub async fn bundle_ancestry(&self, id: &str) -> Result<Vec<BundleImportProvenance>> {
        let mut view = self.read_view().await?;
        let result = view.bundle_ancestry(id).await?;
        view.finish().await?;
        Ok(result)
    }
    pub async fn bundle_installation(&self, id: &str) -> Result<BundleInstallation> {
        let mut view = self.read_view().await?;
        let result = view.bundle_installation(id).await?;
        view.finish().await?;
        Ok(result)
    }
    pub async fn bundle_installations(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<Vec<BundleInstallation>> {
        let mut view = self.read_view().await?;
        let rows=sqlx::query("SELECT * FROM bundle_installations WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,id").bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *view.tx).await.map_err(storage_error)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(decode(&mut view.tx, row).await?);
        }
        view.finish().await?;
        Ok(result)
    }
    pub async fn bundle_resources(&self, id: &str) -> Result<Vec<BundleResource>> {
        let mut view = self.read_view().await?;
        view.bundle_installation(id).await?;
        let rows=sqlx::query("SELECT provenance FROM bundle_resources WHERE installation_id=? ORDER BY kind,original_id,revision").bind(id).fetch_all(&mut *view.tx).await.map_err(storage_error)?;
        let result = rows
            .into_iter()
            .map(|row| serde_json::from_str(row.get("provenance")).map_err(Error::from))
            .collect::<Result<_>>()?;
        view.finish().await?;
        Ok(result)
    }
}
macro_rules! bundle_reads {($owner:ty)=>{impl $owner {
    pub async fn bundle_installation(&mut self,id:&str)->Result<BundleInstallation>{read(&mut self.tx,id).await}
    pub async fn bundle_target_active(&mut self,ns:&NamespaceId,subject:&SubjectId,target:&RetrievalTarget)->Result<bool>{active(&mut self.tx,ns,subject,target).await}
    pub async fn bundle_ancestry(&mut self,id:&str)->Result<Vec<BundleImportProvenance>>{
        sqlx::query("SELECT body FROM bundle_ancestry WHERE installation_id=? ORDER BY source_installation_id").bind(id).fetch_all(&mut *self.tx).await.map_err(storage_error)?.into_iter().map(|v|serde_json::from_str(v.get("body")).map_err(Error::from)).collect()
    }
    pub async fn bundle_vector_dispositions(&mut self,id:&str)->Result<Vec<BundleVectorDisposition>>{
        sqlx::query("SELECT body FROM bundle_vector_dispositions WHERE installation_id=? ORDER BY space_id,source_hash,source_key").bind(id).fetch_all(&mut *self.tx).await.map_err(storage_error)?.into_iter().map(|v|serde_json::from_str(v.get("body")).map_err(Error::from)).collect()
    }
}};}
bundle_reads!(ReadView);
bundle_reads!(WriteTransaction);
impl WriteTransaction {
    pub(super) async fn record_vector_disposition(
        &mut self,
        id: &str,
        value: &BundleVectorDisposition,
    ) -> Result<()> {
        sqlx::query("INSERT INTO bundle_vector_dispositions(installation_id,space_id,source_hash,source_key,body) VALUES(?,?,?,?,?)").bind(id).bind(&value.space_id).bind(&value.source_hash).bind(value.source_target.source_key(&value.source_hash)?).bind(serde_json::to_string(value)?).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub(super) async fn bundle_by_package(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        package: &PackageDescriptor,
        placement: &BundlePlacement,
        digest: &str,
    ) -> Result<Option<BundleInstallation>> {
        let rows=sqlx::query("SELECT * FROM bundle_installations WHERE namespace_id=? AND subject_id=? AND package_id=? AND package_version IS ? AND placement_kind=? AND conversation_ref IS ? ORDER BY commit_seq DESC,id DESC")
            .bind(ns.as_str()).bind(subject.as_str()).bind(&package.id).bind(&package.version).bind(placement.kind()).bind(placement.conversation().map(ConversationRef::as_str)).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let mut active = None;
        for row in rows {
            let installed = decode(&mut self.tx, row).await?;
            if installed.content_digest != digest {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "package release is registered with different content in this placement",
                ));
            }
            if installed.uninstalled_at.is_none() {
                active = Some(installed);
            }
        }
        Ok(active)
    }
    pub(super) async fn register_bundle(&mut self, v: &BundleInstallation) -> Result<()> {
        sqlx::query("INSERT INTO bundle_installations(id,namespace_id,subject_id,source_namespace_id,source_subject_id,package_json,content_level,placement_json,content_digest,archive_material_id,installed_at,commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&v.id).bind(v.namespace_id.as_str()).bind(v.subject_id.as_str()).bind(v.source_namespace_id.as_str()).bind(v.source_subject_id.as_str()).bind(serde_json::to_string(&v.package)?).bind(crate::codec::enum_name(&v.content_level)?).bind(serde_json::to_string(&v.placement)?).bind(&v.content_digest).bind(v.archive_material_id.as_str()).bind(&v.installed_at).bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub(super) async fn inherit_bundle_provenance(
        &mut self,
        id: &str,
        values: &[BundleImportProvenance],
    ) -> Result<()> {
        for source in values {
            sqlx::query("INSERT INTO bundle_ancestry(installation_id,source_installation_id,body) VALUES(?,?,?)").bind(id).bind(&source.installation.id).bind(serde_json::to_string(source)?).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn resource(
        &mut self,
        installation: &BundleInstallation,
        kind: &str,
        original: &str,
        local: &str,
        revision: Option<i64>,
        recorded: &str,
        sequence: i64,
        event: Option<Observation>,
    ) -> Result<()> {
        let resource = BundleResource {
            installation_id: installation.id.clone(),
            kind: kind.into(),
            original_id: original.into(),
            local_id: local.into(),
            revision,
            original_recorded_at: recorded.into(),
            original_commit_seq: sequence,
            original_observation: event,
            original_exposures: vec![],
        };
        sqlx::query("INSERT INTO bundle_resources(installation_id,kind,original_id,local_id,revision,provenance) VALUES(?,?,?,?,?,?)").bind(&installation.id).bind(kind).bind(original).bind(local).bind(revision.unwrap_or(0)).bind(serde_json::to_string(&resource)?).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub(super) async fn record_bundle_exposures(
        &mut self,
        id: &str,
        values: &[MediaExposure],
    ) -> Result<()> {
        for value in values {
            let body:String=sqlx::query_scalar("SELECT provenance FROM bundle_resources WHERE installation_id=? AND kind='observation' AND original_id=?").bind(id).bind(value.observation_id.as_str()).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
            let mut resource: BundleResource = serde_json::from_str(&body)?;
            resource.original_exposures.push(value.clone());
            sqlx::query("UPDATE bundle_resources SET provenance=? WHERE installation_id=? AND kind='observation' AND original_id=?").bind(serde_json::to_string(&resource)?).bind(id).bind(value.observation_id.as_str()).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        Ok(())
    }
    pub async fn withdraw_bundle(&mut self, id: &str) -> Result<BundleWithdrawal> {
        let installation = self.bundle_installation(id).await?;
        self.context()
            .require_subject(&installation.namespace_id, &installation.subject_id)?;
        let mut result = BundleWithdrawal {
            installation,
            withdrawn: vec![],
            retained_modified: vec![],
            already_retired: vec![],
        };
        if result.installation.uninstalled_at.is_some() {
            return Ok(result);
        }
        let ns = result.installation.namespace_id.clone();
        let subject = result.installation.subject_id.clone();
        let rows=sqlx::query("SELECT kind,local_id,max(revision) AS revision,json_group_array(revision) AS revisions FROM bundle_resources WHERE installation_id=? GROUP BY kind,local_id ORDER BY kind,local_id").bind(id).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let kind: String = row.get("kind");
            let local: String = row.get("local_id");
            let revision: i64 = row.get("revision");
            let revisions: Vec<i64> = serde_json::from_str(row.get("revisions"))?;
            match kind.as_str() {
                "memory" => {
                    let mut value = match self.memory(&ns, &MemoryId(local.clone())).await {
                        Ok(value) => value,
                        Err(error) if error.code == ErrorCode::NotFound => {
                            result.already_retired.push(local);
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    if value.revision != revision {
                        result.retained_modified.push(local);
                        continue;
                    }
                    for revision in revisions {
                        result
                            .withdrawn
                            .push(RetrievalTarget::Cognition(CognitiveRef {
                                kind: CognitiveKind::Memory,
                                id: local.clone(),
                                revision,
                            }));
                    }
                    if !value.deleted {
                        value.revision += 1;
                        value.deleted = true;
                        value.commit_seq = self.commit_seq();
                        value.recorded_at = self.time().recorded();
                        self.save_memory(&value, Some(revision)).await?;
                    }
                }
                "cognition" => {
                    let mut value = self
                        .structured_object(&ns, &CognitiveId(local.clone()))
                        .await?;
                    if value.revision != revision {
                        result.retained_modified.push(local);
                        continue;
                    }
                    for revision in revisions {
                        result
                            .withdrawn
                            .push(RetrievalTarget::Cognition(CognitiveRef {
                                kind: value.kind(),
                                id: local.clone(),
                                revision,
                            }));
                    }
                    if !value.deleted {
                        value.revision += 1;
                        value.deleted = true;
                        value.commit_seq = self.commit_seq();
                        value.recorded_at = self.time().recorded();
                        self.save_structured(&value, Some(revision)).await?;
                    }
                }
                "material" => {
                    result
                        .withdrawn
                        .push(RetrievalTarget::Media(MediaReference::Original {
                            material_id: MaterialId(local),
                        }))
                }
                "source_region" => {
                    result
                        .withdrawn
                        .push(RetrievalTarget::Media(MediaReference::SourceRegion {
                            id: SourceRegionId(local),
                        }))
                }
                "representation" => {
                    for revision in revisions {
                        result.withdrawn.push(RetrievalTarget::Media(
                            MediaReference::Representation {
                                reference: RepresentationRef {
                                    id: DerivedRepresentationId(local.clone()),
                                    revision,
                                },
                            },
                        ));
                    }
                }
                "derived_region" => {
                    result
                        .withdrawn
                        .push(RetrievalTarget::Media(MediaReference::DerivedRegion {
                            id: DerivedRegionId(local),
                        }))
                }
                _ => {}
            }
        }
        let recorded = self.time().recorded();
        sqlx::query("UPDATE bundle_installations SET uninstalled_at=? WHERE id=? AND uninstalled_at IS NULL").bind(recorded).bind(id).execute(&mut *self.tx).await.map_err(write_error)?;
        let mut withdrawn = Vec::new();
        for target in result.withdrawn {
            if !self.bundle_target_active(&ns, &subject, &target).await? {
                withdrawn.push(target);
            }
        }
        self.invalidate_context_history(&ns, &subject, &withdrawn)
            .await?;
        self.retire_embedding_targets(&ns, &subject, &withdrawn)
            .await?;
        result.withdrawn = withdrawn;
        result.installation = self.bundle_installation(id).await?;
        Ok(result)
    }
}
async fn read(conn: &mut SqliteConnection, id: &str) -> Result<BundleInstallation> {
    let row = sqlx::query("SELECT * FROM bundle_installations WHERE id=?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    decode(conn, row).await
}
async fn decode(conn: &mut SqliteConnection, row: SqliteRow) -> Result<BundleInstallation> {
    let id: String = row.get("id");
    let counts = sqlx::query(
        "SELECT kind,count(DISTINCT local_id) AS count FROM bundle_resources WHERE installation_id=? GROUP BY kind",
    )
    .bind(&id)
    .fetch_all(conn)
    .await
    .map_err(storage_error)?;
    let mut resource_counts = BTreeMap::new();
    for v in counts {
        resource_counts.insert(v.get("kind"), v.get::<i64, _>("count") as usize);
    }
    Ok(BundleInstallation {
        id,
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        source_namespace_id: NamespaceId(row.get("source_namespace_id")),
        source_subject_id: SubjectId(row.get("source_subject_id")),
        package: serde_json::from_str(row.get("package_json"))?,
        content_level: crate::codec::parse_enum(row.get("content_level"))?,
        placement: serde_json::from_str(row.get("placement_json"))?,
        content_digest: row.get("content_digest"),
        archive_material_id: MaterialId(row.get("archive_material_id")),
        installed_at: row.get("installed_at"),
        uninstalled_at: row.get("uninstalled_at"),
        commit_seq: row.get("commit_seq"),
        resource_counts,
    })
}
pub(crate) async fn active(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    target: &RetrievalTarget,
) -> Result<bool> {
    let (kind, id) = match target {
        RetrievalTarget::Cognition(reference) => (
            if reference.kind == CognitiveKind::Memory {
                "memory"
            } else {
                "cognition"
            },
            reference.id.clone(),
        ),
        RetrievalTarget::Media(MediaReference::Original { material_id }) => {
            ("material", material_id.to_string())
        }
        RetrievalTarget::Media(MediaReference::SourceRegion { id }) => {
            ("source_region", id.to_string())
        }
        RetrievalTarget::Media(MediaReference::Representation { reference }) => {
            ("representation", reference.id.to_string())
        }
        RetrievalTarget::Media(MediaReference::DerivedRegion { id }) => {
            ("derived_region", id.to_string())
        }
    };
    let rows=sqlx::query("SELECT i.uninstalled_at,r.revision FROM bundle_resources r JOIN bundle_installations i ON i.id=r.installation_id WHERE i.namespace_id=? AND i.subject_id=? AND r.kind=? AND r.local_id=?").bind(ns.as_str()).bind(subject.as_str()).bind(kind).bind(&id).fetch_all(&mut *conn).await.map_err(storage_error)?;
    if rows.is_empty()
        || rows
            .iter()
            .any(|v| v.get::<Option<String>, _>("uninstalled_at").is_none())
    {
        return Ok(true);
    }
    match target {
        RetrievalTarget::Cognition(reference) => {
            let max = rows
                .iter()
                .map(|v| v.get::<i64, _>("revision"))
                .max()
                .unwrap_or(0);
            // An explicit later revision belongs to the operator, including a later restore.
            let (current, deleted) = if reference.kind == CognitiveKind::Memory {
                let value = crate::cognition::memory::memory(conn, ns, &MemoryId(id)).await?;
                (value.revision, value.deleted)
            } else {
                let value =
                    crate::cognition::structured::read::object(conn, ns, &CognitiveId(id), None)
                        .await?;
                (value.revision, value.deleted)
            };
            Ok(current > max && !deleted)
        }
        RetrievalTarget::Media(reference) => {
            if let MediaReference::Representation { reference } = reference {
                let maximum = rows
                    .iter()
                    .map(|v| v.get::<i64, _>("revision"))
                    .max()
                    .unwrap_or(0);
                if crate::media::head_in(conn, ns, subject, &reference.id)
                    .await?
                    .is_some_and(|value| value.revision > maximum)
                {
                    return Ok(true);
                }
            }
            let roots = crate::media::roots_in(conn, ns, subject, reference).await?;
            for material in roots {
                let independent:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM observations o WHERE o.namespace_id=? AND o.subject_id=? AND o.material_id=? AND NOT EXISTS(SELECT 1 FROM bundle_resources r JOIN bundle_installations i ON i.id=r.installation_id WHERE r.kind='observation' AND r.local_id=o.id AND i.uninstalled_at IS NOT NULL))").bind(ns.as_str()).bind(subject.as_str()).bind(material.as_str()).fetch_one(&mut *conn).await.map_err(storage_error)?;
                if independent {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}
