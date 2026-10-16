//! Subject identity, initialization material, configuration and lifecycle.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod seed;

use chrono::{DateTime, Utc};
use nous_configuration::{
    ConfigurationService, process_capabilities, subject_default_capabilities,
};
use nous_core::{Error, OperationId, Result, SubjectId};
use nous_object_store::ObjectStore;
use nous_persistence::AuthorityStore;
use serde::{Deserialize, Serialize};
use sqlx::Row;

pub use nous_configuration::SubjectCapabilities;
pub use seed::{
    COGNITIVE_SEED_FORMAT, CognitiveSeedInput, CognitiveSeedSource, CognitiveSeedVersion,
    CognitiveSeedView, MARKDOWN_SEED_FORMAT, SeedAdoptionKind, SubjectSeedAdoption,
};

#[derive(Clone)]
pub struct SubjectCoreService {
    pub(crate) store: AuthorityStore,
    pub(crate) objects: ObjectStore,
    pub(crate) configuration: ConfigurationService,
    pub(crate) clock: std::sync::Arc<dyn nous_runtime::CognitiveClock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSubject {
    pub subject_id: Option<SubjectId>,
    pub operation_id: OperationId,
    pub cognitive_seed: Option<CognitiveSeedInput>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub capabilities: Option<SubjectCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectView {
    pub subject_id: SubjectId,
    pub created_at: DateTime<Utc>,
    pub authority_seq: i64,
    pub status: String,
    pub metadata: serde_json::Value,
    pub capabilities: SubjectCapabilities,
}

impl SubjectCoreService {
    pub fn new(
        store: AuthorityStore,
        objects: ObjectStore,
        configuration: ConfigurationService,
    ) -> Self {
        Self::with_clock(
            store,
            objects,
            configuration,
            std::sync::Arc::new(nous_runtime::SystemCognitiveClock),
        )
    }

    pub fn with_clock(
        store: AuthorityStore,
        objects: ObjectStore,
        configuration: ConfigurationService,
        clock: std::sync::Arc<dyn nous_runtime::CognitiveClock>,
    ) -> Self {
        Self {
            clock,
            store,
            objects,
            configuration,
        }
    }

    pub async fn create_subject(&self, input: CreateSubject) -> Result<SubjectView> {
        if input.operation_id.0.is_nil() {
            return Err(Error::Invalid("operation_id is required".into()));
        }
        let subject = input.subject_id.unwrap_or(SubjectId(input.operation_id.0));
        let digest = nous_core::canonical_request_digest("subject.create", subject, &input)?;
        let guard = self.objects.reference_guard(false).await?;
        let mut tx = self.store.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("subject.create:{}", input.operation_id.0))
            .execute(&mut *tx)
            .await
            .map_err(nous_persistence::database_error)?;
        if let Some(row) = sqlx::query("SELECT subject_id,request_digest FROM mutation_receipts WHERE operation_id=$1 AND operation_kind='subject.create'")
            .bind(input.operation_id.0).fetch_optional(&mut *tx).await.map_err(nous_persistence::database_error)? {
            let saved_subject: uuid::Uuid = row.try_get("subject_id").map_err(nous_persistence::database_error)?;
            if saved_subject != subject.0 || row.try_get::<String,_>("request_digest").map_err(nous_persistence::database_error)? != digest {
                return Err(nous_core::DomainError::new(nous_core::DomainErrorCode::OperationIdConflict,"Subject creation operation_id already binds different input").with_context("operation_id",input.operation_id.0).into());
            }
            tx.commit().await.map_err(nous_persistence::database_error)?;
            drop(guard);
            return self.subject(subject).await;
        }
        let snapshot = self.configuration.active_system_snapshot()?;
        let process = process_capabilities(&snapshot)?;
        let capabilities = input
            .capabilities
            .unwrap_or(subject_default_capabilities(&snapshot)?);
        let capabilities = capabilities.require_process(process)?;
        sqlx::query("INSERT INTO subjects(subject_id,created_at,metadata) VALUES($1,$2,$3)")
            .bind(subject.0)
            .bind(self.clock.now(subject))
            .bind(input.metadata)
            .execute(&mut *tx)
            .await
            .map_err(nous_persistence::database_error)?;
        sqlx::query(
            "INSERT INTO subject_capabilities(subject_id,memory,self_cognition,social_cognition,motivation) VALUES($1,$2,$3,$4,$5)",
        )
        .bind(subject.0)
        .bind(capabilities.memory)
        .bind(capabilities.self_cognition)
        .bind(capabilities.social_cognition)
        .bind(capabilities.motivation)
        .execute(&mut *tx)
        .await
        .map_err(nous_persistence::database_error)?;
        nous_persistence::check_receipt(
            &mut tx,
            subject,
            input.operation_id,
            "subject.create",
            &digest,
        )
        .await?;
        self.store
            .ensure_identity_addresses_in(
                &mut tx,
                subject,
                &[nous_core::CognitiveRef::Subject(subject)],
                "",
            )
            .await?;
        if let Some(cognitive_seed) = input.cognitive_seed {
            seed::insert_seed(
                &self.store,
                &self.objects,
                &mut tx,
                cognitive_seed,
                seed::SeedAdoptionWrite {
                    subject,
                    operation_id: input.operation_id,
                    kind: SeedAdoptionKind::Initial,
                    digest: &digest,
                    now: self.clock.now(subject),
                },
            )
            .await?;
        }
        nous_persistence::commit_receipt(
            &mut tx,
            subject,
            input.operation_id,
            "subject",
            Some(&subject.0.to_string()),
            None,
            None,
        )
        .await?;
        tx.commit()
            .await
            .map_err(nous_persistence::database_error)?;
        drop(guard);
        self.subject(subject).await
    }

    pub async fn subject(&self, subject: SubjectId) -> Result<SubjectView> {
        let row = sqlx::query("SELECT s.subject_id,s.created_at,s.authority_seq,s.status,s.metadata,c.memory,c.self_cognition,c.social_cognition,c.motivation FROM subjects s JOIN subject_capabilities c USING(subject_id) WHERE s.subject_id=$1")
            .bind(subject.0).fetch_optional(self.store.pool()).await
            .map_err(nous_persistence::database_error)?
            .ok_or_else(|| Error::NotFound("subject not found".into()))?;
        decode_subject(row)
    }

    pub async fn subjects(
        &self,
        status: &str,
        limit: i64,
        after: Option<uuid::Uuid>,
    ) -> Result<(Vec<SubjectView>, bool)> {
        let mut rows = sqlx::query("SELECT s.subject_id,s.created_at,s.authority_seq,s.status,s.metadata,c.memory,c.self_cognition,c.social_cognition,c.motivation FROM subjects s JOIN subject_capabilities c USING(subject_id) WHERE ($1::uuid IS NULL OR s.subject_id>$1) AND ($2='' OR s.status=$2) ORDER BY s.subject_id LIMIT $3")
            .bind(after).bind(status).bind(limit+1).fetch_all(self.store.pool()).await.map_err(nous_persistence::database_error)?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        Ok((
            rows.into_iter()
                .map(decode_subject)
                .collect::<Result<_>>()?,
            more,
        ))
    }
}

fn decode_subject(row: sqlx::postgres::PgRow) -> Result<SubjectView> {
    Ok(SubjectView {
        subject_id: SubjectId(
            row.try_get("subject_id")
                .map_err(nous_persistence::database_error)?,
        ),
        created_at: row
            .try_get("created_at")
            .map_err(nous_persistence::database_error)?,
        authority_seq: row
            .try_get("authority_seq")
            .map_err(nous_persistence::database_error)?,
        status: row
            .try_get("status")
            .map_err(nous_persistence::database_error)?,
        metadata: row
            .try_get("metadata")
            .map_err(nous_persistence::database_error)?,
        capabilities: SubjectCapabilities {
            motivation: row
                .try_get("motivation")
                .map_err(nous_persistence::database_error)?,
            social_cognition: row
                .try_get("social_cognition")
                .map_err(nous_persistence::database_error)?,
            self_cognition: row
                .try_get("self_cognition")
                .map_err(nous_persistence::database_error)?,
            memory: row
                .try_get("memory")
                .map_err(nous_persistence::database_error)?,
        },
    })
}

impl SubjectCoreService {
    /// Provision additional requested domains without resetting existing cognition.
    pub async fn provision_capabilities(
        &self,
        subject: SubjectId,
        operation: OperationId,
        requested: SubjectCapabilities,
    ) -> Result<SubjectView> {
        if operation.0.is_nil() {
            return Err(Error::Invalid("operation_id is required".into()));
        }
        let process = process_capabilities(&self.configuration.active_system_snapshot()?)?;
        requested.require_process(process)?;
        let digest = nous_core::canonical_request_digest("subject.provision", subject, &requested)?;
        let mut mutation = match nous_persistence::MutationEnvelope::begin(
            &self.store,
            subject,
            operation,
            "subject.provision",
            &digest,
            Some(nous_persistence::OwnerLock("subject-capabilities")),
        )
        .await?
        {
            nous_persistence::MutationStart::Replay(_) => return self.subject(subject).await,
            nous_persistence::MutationStart::Active(m) => m,
        };
        let changed=sqlx::query("UPDATE subject_capabilities SET memory=memory OR $2,self_cognition=self_cognition OR $3,social_cognition=social_cognition OR $4,motivation=motivation OR $5 WHERE subject_id=$1 AND (($2 AND NOT memory) OR ($3 AND NOT self_cognition) OR ($4 AND NOT social_cognition) OR ($5 AND NOT motivation))").bind(subject.0).bind(requested.memory).bind(requested.self_cognition).bind(requested.social_cognition).bind(requested.motivation).execute(&mut **mutation.tx()).await.map_err(nous_persistence::database_error)?.rows_affected();
        if changed > 0 {
            mutation
                .invalidate(nous_persistence::ProjectionInvalidation::all())
                .await?;
        }
        mutation
            .commit("subject", Some(&subject.0.to_string()), None, None)
            .await?;
        self.subject(subject).await
    }
}
