//! Complete file outcomes and their current deduplication qualifications.
use crate::{ReadView, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::*;
use serde::de::DeserializeOwned;
use sqlx::SqliteConnection;
use std::collections::BTreeSet;

pub struct ImportFileLookup<'a> {
    pub namespace: &'a NamespaceId,
    pub subject: &'a SubjectId,
    pub origin: &'a Option<ObservationOrigin>,
    pub policy: ImportFileDedupePolicy,
    pub vectors: ImportVectorPolicy,
    pub key: &'a ImportManifestKey,
    pub content_hash: &'a str,
}
async fn read<T: DeserializeOwned>(
    conn: &mut SqliteConnection,
    table: &str,
    ns: &NamespaceId,
    sub: &SubjectId,
    id: &str,
    cutoff: i64,
) -> Result<T> {
    // Table identifiers are private constants at the call sites.
    let body: Option<String> = sqlx::query_scalar(&format!(
        "SELECT body FROM {table} WHERE namespace_id=? AND subject_id=? AND id=? AND commit_seq<=?"
    ))
    .bind(ns.as_str())
    .bind(sub.as_str())
    .bind(id)
    .bind(cutoff)
    .fetch_optional(conn)
    .await
    .map_err(storage_error)?;
    Ok(serde_json::from_str(&body.ok_or_else(not_found)?)?)
}
fn origin_key(origin: &Option<ObservationOrigin>) -> Result<String> {
    if let Some(origin) = origin {
        origin.validate()?;
    }
    Ok(serde_json::to_string(origin)?)
}
fn policy_key(policy: ImportFileDedupePolicy) -> Result<&'static str> {
    match policy {
        ImportFileDedupePolicy::ContentHash => Ok("content_hash"),
        ImportFileDedupePolicy::Manifest => Ok("manifest"),
        ImportFileDedupePolicy::None => Err(Error::invalid("dedupe none has no manifest head")),
    }
}
fn manifest_key(key: &ImportManifestKey) -> Result<String> {
    match key {
        ImportManifestKey::ContentHash { hash } => {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err(Error::invalid(
                    "manifest content hash requires canonical SHA256",
                ));
            }
        }
        ImportManifestKey::CallerFile { key } => validate_identity(key)?,
    }
    Ok(serde_json::to_string(key)?)
}
fn validate_identity(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 4096 || key.chars().any(char::is_control) {
        return Err(Error::invalid(
            "logical file identity requires 1..4096 bytes without controls",
        ));
    }
    Ok(())
}
impl WriteTransaction {
    pub async fn insert_import_file_success(&mut self, fact: &ImportFileSuccess) -> Result<()> {
        self.check_sequence(fact.commit_seq)?;
        fact.id.validate()?;
        self.context()
            .require_subject(&fact.namespace_id, &fact.subject_id)?;
        validate_name(&fact.processing_job_id, "historical file processing job")?;
        origin_key(&fact.origin)?;
        if let Some(key) = &fact.file_identity {
            validate_identity(key)?;
        }
        let catalog: ImportUnitCatalog = read(
            &mut self.tx,
            "import_unit_catalogs",
            &fact.namespace_id,
            &fact.subject_id,
            fact.catalog_id.as_str(),
            i64::MAX,
        )
        .await?;
        let document = catalog
            .documents
            .iter()
            .find(|document| document.input_source_id == fact.input_source_id)
            .ok_or_else(|| Error::invalid("file success leaves its frozen catalog"))?;
        if fact.unit_outputs.is_empty() || fact.unit_outputs.len() != document.original_unit_count {
            return Err(Error::invalid(
                "file success requires its complete original unit set",
            ));
        }
        let mut units = BTreeSet::new();
        let mut outputs = BTreeSet::new();
        for pair in &fact.unit_outputs {
            if !units.insert(pair.unit_id.clone()) || !outputs.insert(pair.output_id.clone()) {
                return Err(Error::invalid("file success repeats a unit or output"));
            }
            let unit: ImportTextUnit = read(
                &mut self.tx,
                "import_text_units",
                &fact.namespace_id,
                &fact.subject_id,
                pair.unit_id.as_str(),
                i64::MAX,
            )
            .await?;
            let output: ImportUnitOutput = read(
                &mut self.tx,
                "import_unit_outputs",
                &fact.namespace_id,
                &fact.subject_id,
                pair.output_id.as_str(),
                i64::MAX,
            )
            .await?;
            if unit.catalog_id != fact.catalog_id
                || unit.input_source_id != fact.input_source_id
                || output.unit_id != unit.id
            {
                return Err(Error::invalid(
                    "file success output differs from its actual document unit",
                ));
            }
        }
        // Local execution checks successful attempts before this call. Imported history has no local Job FK.
        sqlx::query("INSERT INTO import_file_successes(id,body) VALUES(?,?)")
            .bind(fact.id.as_str())
            .bind(serde_json::to_string(fact)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn set_import_manifest_head(&mut self, head: &ImportManifestHead) -> Result<()> {
        let (origin, key) = self.validate_import_manifest_head(head).await?;
        sqlx::query("INSERT INTO import_manifest_heads(namespace_id,subject_id,origin_key,policy,manifest_key,body) VALUES(?,?,?,?,?,?) ON CONFLICT(namespace_id,subject_id,origin_key,policy,manifest_key) DO UPDATE SET body=excluded.body")
            .bind(head.namespace_id.as_str()).bind(head.subject_id.as_str()).bind(origin).bind(policy_key(head.policy)?).bind(key).bind(serde_json::to_string(head)?)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    async fn validate_import_manifest_head(
        &mut self,
        head: &ImportManifestHead,
    ) -> Result<(String, String)> {
        self.check_sequence(head.commit_seq)?;
        self.context()
            .require_subject(&head.namespace_id, &head.subject_id)?;
        let origin = origin_key(&head.origin)?;
        let key = manifest_key(&head.key)?;
        let fact: ImportFileSuccess = read(
            &mut self.tx,
            "import_file_successes",
            &head.namespace_id,
            &head.subject_id,
            head.success_id.as_str(),
            i64::MAX,
        )
        .await?;
        let input = self
            .import_input_source(&head.namespace_id, &head.subject_id, &fact.input_source_id)
            .await?;
        if fact.origin != head.origin
            || fact.dedupe_policy != head.policy
            || head
                .policy
                .key(&input.content_hash, fact.file_identity.as_deref())
                .as_ref()
                != Some(&head.key)
            || match &head.key {
                ImportManifestKey::ContentHash { hash } => *hash != input.content_hash,
                ImportManifestKey::CallerFile { key } => fact.file_identity.as_ref() != Some(key),
            }
        {
            return Err(Error::invalid(
                "manifest qualification differs from actual file success",
            ));
        }
        Ok((origin, key))
    }
    /// Native merge accepts an incoming qualification only if the local key is absent.
    pub async fn insert_import_manifest_head_if_absent(
        &mut self,
        head: &ImportManifestHead,
    ) -> Result<bool> {
        let (origin, key) = self.validate_import_manifest_head(head).await?;
        Ok(sqlx::query("INSERT INTO import_manifest_heads(namespace_id,subject_id,origin_key,policy,manifest_key,body) VALUES(?,?,?,?,?,?) ON CONFLICT(namespace_id,subject_id,origin_key,policy,manifest_key) DO NOTHING")
            .bind(head.namespace_id.as_str()).bind(head.subject_id.as_str()).bind(origin).bind(policy_key(head.policy)?).bind(key).bind(serde_json::to_string(head)?)
            .execute(&mut *self.tx).await.map_err(write_error)?.rows_affected()==1)
    }
    pub async fn clear_import_manifest_heads(
        &mut self,
        ns: &NamespaceId,
        sub: &SubjectId,
        origin: &Option<ObservationOrigin>,
    ) -> Result<u64> {
        self.context().require_subject(ns, sub)?;
        Ok(sqlx::query("DELETE FROM import_manifest_heads WHERE namespace_id=? AND subject_id=? AND origin_key=?")
            .bind(ns.as_str()).bind(sub.as_str()).bind(origin_key(origin)?).execute(&mut *self.tx).await.map_err(write_error)?.rows_affected())
    }
}
async fn candidate(
    conn: &mut SqliteConnection,
    lookup: &ImportFileLookup<'_>,
    cutoff: i64,
) -> Result<Option<(ImportFileSuccess, Vec<SourceReadRef>, Vec<MemoryId>)>> {
    let ImportFileLookup {
        namespace: ns,
        subject: sub,
        origin,
        policy,
        key,
        content_hash: hash,
        vectors,
    } = lookup;
    let id: Option<String>=sqlx::query_scalar("SELECT success_id FROM import_manifest_heads WHERE namespace_id=? AND subject_id=? AND origin_key=? AND policy=? AND manifest_key=? AND commit_seq<=?")
        .bind(ns.as_str()).bind(sub.as_str()).bind(origin_key(origin)?).bind(policy_key(*policy)?).bind(manifest_key(key)?).bind(cutoff)
        .fetch_optional(&mut *conn).await.map_err(storage_error)?;
    let Some(id) = id else { return Ok(None) };
    let fact: ImportFileSuccess = read(conn, "import_file_successes", ns, sub, &id, cutoff).await?;
    if *vectors == ImportVectorPolicy::Required
        && fact
            .unit_outputs
            .iter()
            .any(|pair| pair.vectors != ImportVectorPolicy::Required)
    {
        return Ok(None);
    }
    let input: ImportInputSource = read(
        conn,
        "import_input_sources",
        ns,
        sub,
        fact.input_source_id.as_str(),
        cutoff,
    )
    .await?;
    if input.content_hash != *hash {
        return Ok(None);
    }
    let mut sources = BTreeSet::new();
    let mut memories = BTreeSet::new();
    for pair in &fact.unit_outputs {
        let unit: ImportTextUnit = read(
            conn,
            "import_text_units",
            ns,
            sub,
            pair.unit_id.as_str(),
            cutoff,
        )
        .await?;
        sources.extend(unit.source_requirements());
        let output: ImportUnitOutput = read(
            conn,
            "import_unit_outputs",
            ns,
            sub,
            pair.output_id.as_str(),
            cutoff,
        )
        .await?;
        memories.extend(output.memory_ids);
    }
    Ok(Some((
        fact,
        sources.into_iter().collect(),
        memories.into_iter().collect(),
    )))
}
macro_rules! reader {
    ($owner:ty,$cutoff:expr) => {
        impl $owner {
            pub async fn import_file_success(
                &mut self,
                ns: &NamespaceId,
                sub: &SubjectId,
                id: &ImportFileSuccessId,
            ) -> Result<ImportFileSuccess> {
                let cutoff = ($cutoff)(self);
                read(
                    &mut self.tx,
                    "import_file_successes",
                    ns,
                    sub,
                    id.as_str(),
                    cutoff,
                )
                .await
            }
            pub async fn import_manifest_head(&mut self,ns:&NamespaceId,sub:&SubjectId,origin:&Option<ObservationOrigin>,policy:ImportFileDedupePolicy,key:&ImportManifestKey)->Result<Option<ImportManifestHead>> {
                if policy==ImportFileDedupePolicy::None {return Ok(None)}
                let cutoff=($cutoff)(self);
                let body:Option<String>=sqlx::query_scalar("SELECT body FROM import_manifest_heads WHERE namespace_id=? AND subject_id=? AND origin_key=? AND policy=? AND manifest_key=? AND commit_seq<=?")
                    .bind(ns.as_str()).bind(sub.as_str()).bind(origin_key(origin)?).bind(policy_key(policy)?).bind(manifest_key(key)?).bind(cutoff)
                    .fetch_optional(&mut *self.tx).await.map_err(storage_error)?;
                body.map(|body|serde_json::from_str(&body).map_err(Error::from)).transpose()
            }
            pub async fn eligible_import_file_success(
                &mut self,
                ctx: &CallContext,
                lookup: &ImportFileLookup<'_>,
            ) -> Result<Option<ImportFileSuccess>> {
                let (ns, sub) = (lookup.namespace, lookup.subject);
                ctx.require_subject(ns, sub)?;
                if lookup.policy == ImportFileDedupePolicy::None {
                    return Ok(None);
                }
                let cutoff = ($cutoff)(self);
                let Some((fact, sources, memories)) =
                    candidate(&mut self.tx, lookup, cutoff).await?
                else {
                    return Ok(None);
                };
                match self
                    .require_source_requirements(ctx, ns, sub, &sources)
                    .await
                {
                    Ok(()) => {}
                    Err(error) if error.code == ErrorCode::Forbidden => return Ok(None),
                    Err(error) => return Err(error),
                }
                for id in memories {
                    let memory = match self.memory(ns, &id).await {
                        Ok(memory) => memory,
                        Err(error) if error.code == ErrorCode::NotFound => continue,
                        Err(error) => return Err(error),
                    };
                    if memory.subject_id != *sub || memory.deleted {
                        continue;
                    }
                    let target = RetrievalTarget::Cognition(CognitiveRef::memory(
                        &memory.id,
                        memory.revision,
                    ));
                    match self.require_source_read(ctx, ns, sub, &target).await {
                        Ok(()) => return Ok(Some(fact)),
                        Err(error) if error.code == ErrorCode::Forbidden => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(None)
            }
        }
    };
}
reader!(ReadView, |view: &ReadView| view.watermark);
reader!(WriteTransaction, |tx: &WriteTransaction| tx.commit_seq());
impl ReadView {
    pub async fn import_file_facts(
        &mut self,
        ns: &NamespaceId,
        sub: &SubjectId,
    ) -> Result<(Vec<ImportFileSuccess>, Vec<ImportManifestHead>)> {
        let facts:Vec<String>=sqlx::query_scalar("SELECT body FROM import_file_successes WHERE namespace_id=? AND subject_id=? AND commit_seq<=? ORDER BY commit_seq,id")
            .bind(ns.as_str()).bind(sub.as_str()).bind(self.watermark).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let heads:Vec<String>=sqlx::query_scalar("SELECT body FROM import_manifest_heads WHERE namespace_id=? AND subject_id=? AND commit_seq<=? ORDER BY origin_key,policy,manifest_key")
            .bind(ns.as_str()).bind(sub.as_str()).bind(self.watermark).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        Ok((
            facts
                .into_iter()
                .map(|body| serde_json::from_str(&body).map_err(Error::from))
                .collect::<Result<_>>()?,
            heads
                .into_iter()
                .map(|body| serde_json::from_str(&body).map_err(Error::from))
                .collect::<Result<_>>()?,
        ))
    }
}
