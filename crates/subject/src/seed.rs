// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use nous_core::{
    ArtifactId, CognitiveSeedVersionId, Error, OperationId, Result, SubjectId,
    canonical_request_digest,
};
use nous_persistence::database_error as db;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::SubjectCoreService;

pub const COGNITIVE_SEED_FORMAT: &str = "application/vnd.nous-wave.cognitive-seed+toml;version=1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveSeedInput {
    pub text: String,
    #[serde(default = "default_seed_format")]
    pub format: String,
    #[serde(default)]
    pub provenance: serde_json::Value,
}

fn default_seed_format() -> String {
    COGNITIVE_SEED_FORMAT.into()
}

impl CognitiveSeedInput {
    pub fn validate(&self) -> Result<()> {
        if self.text.trim().is_empty() {
            return Err(Error::Invalid("Cognitive Seed content is required".into()));
        }
        if self.format != COGNITIVE_SEED_FORMAT {
            return Err(Error::Invalid(format!(
                "unsupported Cognitive Seed format: {}",
                self.format
            )));
        }
        validate_seed_document(&self.text)?;
        Ok(())
    }
}

fn validate_seed_document(text: &str) -> Result<()> {
    let value: toml::Value = toml::from_str(text)
        .map_err(|error| Error::Invalid(format!("invalid Cognitive Seed TOML: {error}")))?;
    let schema_version = value
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| Error::Invalid("Cognitive Seed schema_version is required".into()))?;
    if schema_version != 1 {
        return Err(Error::Invalid(format!(
            "unsupported Cognitive Seed schema_version: {schema_version}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedAdoptionKind {
    Initial,
    Import,
}

impl SeedAdoptionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Import => "import",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveSeedVersion {
    pub seed_version_id: CognitiveSeedVersionId,
    pub subject_id: SubjectId,
    pub artifact_id: ArtifactId,
    pub format: String,
    pub provenance: serde_json::Value,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectSeedAdoption {
    pub adoption_id: Uuid,
    pub subject_id: SubjectId,
    pub seed_version_id: CognitiveSeedVersionId,
    pub kind: SeedAdoptionKind,
    pub operation_id: OperationId,
    pub request_digest: String,
    pub adopted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveSeedView {
    pub version: CognitiveSeedVersion,
    pub adoption: SubjectSeedAdoption,
    pub text: String,
}

impl SubjectCoreService {
    pub async fn adopt_cognitive_seed(
        &self,
        subject: SubjectId,
        operation_id: OperationId,
        input: CognitiveSeedInput,
        kind: SeedAdoptionKind,
    ) -> Result<CognitiveSeedView> {
        input.validate()?;
        self.subject(subject).await?;
        let guard = self.objects.reference_guard(false).await?;
        let hash = self.objects.put(input.text.as_bytes().to_vec()).await?;
        let digest = canonical_request_digest(
            "cognitive_seed.adopt",
            subject,
            &(&operation_id, &kind, &input),
        )?;
        let mut tx = self.store.begin().await?;
        let adoption = insert_seed(
            &self.store,
            &mut tx,
            input,
            hash,
            SeedAdoptionWrite {
                subject,
                operation_id,
                kind,
                digest: &digest,
                now: self.clock.now(subject),
            },
        )
        .await?;
        tx.commit().await.map_err(db)?;
        drop(guard);
        self.cognitive_seed(subject, adoption).await
    }

    pub async fn cognitive_seed(
        &self,
        subject: SubjectId,
        adoption_id: Uuid,
    ) -> Result<CognitiveSeedView> {
        let row = sqlx::query("SELECT a.adoption_id,a.subject_id,a.seed_version_id,a.kind,a.operation_id,a.request_digest,a.adopted_at,v.artifact_id,v.format,v.provenance,v.content_hash,v.created_at FROM subject_seed_adoptions a JOIN cognitive_seed_versions v USING(seed_version_id) WHERE a.subject_id=$1 AND a.adoption_id=$2")
            .bind(subject.0)
            .bind(adoption_id)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("Cognitive Seed adoption not found".into()))?;
        let text = String::from_utf8(
            self.objects
                .get(&row.try_get::<String, _>("content_hash").map_err(db)?)
                .await?,
        )
        .map_err(|error| Error::Infrastructure(error.to_string()))?;
        Ok(CognitiveSeedView {
            version: CognitiveSeedVersion {
                seed_version_id: CognitiveSeedVersionId(
                    row.try_get("seed_version_id").map_err(db)?,
                ),
                subject_id: subject,
                artifact_id: ArtifactId(row.try_get("artifact_id").map_err(db)?),
                format: row.try_get("format").map_err(db)?,
                provenance: row.try_get("provenance").map_err(db)?,
                content_hash: row.try_get("content_hash").map_err(db)?,
                created_at: row.try_get("created_at").map_err(db)?,
            },
            adoption: SubjectSeedAdoption {
                adoption_id: row.try_get("adoption_id").map_err(db)?,
                subject_id: subject,
                seed_version_id: CognitiveSeedVersionId(
                    row.try_get("seed_version_id").map_err(db)?,
                ),
                kind: parse_adoption_kind(row.try_get("kind").map_err(db)?)?,
                operation_id: OperationId(row.try_get("operation_id").map_err(db)?),
                request_digest: row.try_get("request_digest").map_err(db)?,
                adopted_at: row.try_get("adopted_at").map_err(db)?,
            },
            text,
        })
    }

    pub async fn latest_cognitive_seed(&self, subject: SubjectId) -> Result<CognitiveSeedView> {
        let adoption: Uuid = sqlx::query_scalar("SELECT adoption_id FROM subject_seed_adoptions WHERE subject_id=$1 ORDER BY adopted_at DESC,adoption_id DESC LIMIT 1")
            .bind(subject.0)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("Cognitive Seed adoption not found".into()))?;
        self.cognitive_seed(subject, adoption).await
    }
}

pub(super) struct SeedAdoptionWrite<'a> {
    pub subject: SubjectId,
    pub operation_id: OperationId,
    pub kind: SeedAdoptionKind,
    pub digest: &'a str,
    pub now: DateTime<Utc>,
}

pub(super) async fn insert_seed(
    store: &nous_persistence::AuthorityStore,
    tx: &mut Transaction<'_, Postgres>,
    input: CognitiveSeedInput,
    hash: String,
    adoption: SeedAdoptionWrite<'_>,
) -> Result<Uuid> {
    let SeedAdoptionWrite {
        subject,
        operation_id,
        kind,
        digest,
        now,
    } = adoption;
    sqlx::query("SELECT subject_id FROM subjects WHERE subject_id=$1 FOR UPDATE")
        .bind(subject.0)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
    if let Some(row) = sqlx::query("SELECT adoption_id,request_digest FROM subject_seed_adoptions WHERE subject_id=$1 AND operation_id=$2")
        .bind(subject.0)
        .bind(operation_id.0)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
    {
        let existing: String = row.try_get("request_digest").map_err(db)?;
        if existing != digest {
            return Err(nous_core::DomainError::new(nous_core::DomainErrorCode::OperationIdConflict, "Cognitive Seed operation_id was used with a different request")
                .with_context("operation_id", operation_id.0).into());
        }
        return row.try_get("adoption_id").map_err(db);
    }
    let artifact = sqlx::query_scalar::<_, Uuid>("INSERT INTO artifacts(artifact_id,subject_id,content_hash,byte_length,media_type,storage_key,created_at,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,'{}') ON CONFLICT(subject_id,content_hash) DO UPDATE SET content_hash=excluded.content_hash RETURNING artifact_id")
        .bind(Uuid::now_v7())
        .bind(subject.0)
        .bind(&hash)
        .bind(input.text.len() as i64)
        .bind("application/toml")
        .bind(format!("{}/{}", &hash[..2], hash))
        .bind(now)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
    let seed_version = sqlx::query_scalar::<_, Uuid>("INSERT INTO cognitive_seed_versions(seed_version_id,subject_id,artifact_id,format,provenance,content_hash,created_at) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(subject_id,content_hash,format) DO UPDATE SET content_hash=excluded.content_hash RETURNING seed_version_id")
        .bind(Uuid::now_v7())
        .bind(subject.0)
        .bind(artifact)
        .bind(&input.format)
        .bind(input.provenance)
        .bind(&hash)
        .bind(now)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
    let adoption = Uuid::now_v7();
    sqlx::query("INSERT INTO subject_seed_adoptions(adoption_id,subject_id,seed_version_id,kind,operation_id,request_digest,adopted_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(adoption)
        .bind(subject.0)
        .bind(seed_version)
        .bind(kind.as_str())
        .bind(operation_id.0)
        .bind(digest)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    sqlx::query("UPDATE subjects SET authority_seq=authority_seq+1 WHERE subject_id=$1")
        .bind(subject.0)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    store
        .ensure_identity_addresses_in(
            tx,
            subject,
            &[
                nous_core::CognitiveRef::Subject(subject),
                nous_core::CognitiveRef::Artifact(ArtifactId(artifact)),
                nous_core::CognitiveRef::CognitiveSeedVersion(CognitiveSeedVersionId(seed_version)),
            ],
            "",
        )
        .await?;
    Ok(adoption)
}

fn parse_adoption_kind(value: String) -> Result<SeedAdoptionKind> {
    match value.as_str() {
        "initial" => Ok(SeedAdoptionKind::Initial),
        "import" => Ok(SeedAdoptionKind::Import),
        _ => Err(Error::Infrastructure(
            "invalid Cognitive Seed adoption kind".into(),
        )),
    }
}
