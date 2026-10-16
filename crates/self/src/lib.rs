//! Self owns adopted facets and narrative interpretation, never copied Memory.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod context;
mod contract;
mod historical;
mod lifecycle;
mod mutations;
mod query;
mod read;
mod selection;
mod trace;
use chrono::{DateTime, Utc};
pub use contract::*;
use nous_core::*;
use nous_material::MaterialService;
use nous_object_store::ObjectStore;
use nous_persistence::{
    AuthorityStore, MutationEnvelope, MutationStart, OwnerLock, ProjectionInvalidation,
    database_error as db,
};
use nous_runtime::CognitiveRuntimeService;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;
#[derive(Clone)]
pub struct SelfService {
    store: AuthorityStore,
    material: MaterialService,
    objects: ObjectStore,
    runtime: CognitiveRuntimeService,
}
impl SelfService {
    pub fn new(
        store: AuthorityStore,
        material: MaterialService,
        objects: ObjectStore,
        runtime: CognitiveRuntimeService,
    ) -> Self {
        Self {
            store,
            material,
            objects,
            runtime,
        }
    }
    pub async fn require_enabled(&self, subject: SubjectId) -> Result<()> {
        let enabled: Option<bool> = sqlx::query_scalar(
            "SELECT self_cognition FROM subject_capabilities WHERE subject_id=$1",
        )
        .bind(subject.0)
        .fetch_optional(self.store.pool())
        .await
        .map_err(db)?;
        match enabled {
            Some(true) => Ok(()),
            Some(false) => Err(DomainError::new(
                DomainErrorCode::CapabilityUnavailable,
                "Subject Self capability is disabled",
            )
            .with_context("capability", "self_cognition")
            .into()),
            None => Err(Error::NotFound("Subject not found".into())),
        }
    }
    async fn begin(
        &self,
        subject: SubjectId,
        op: OperationId,
        kind: &str,
        digest: &str,
    ) -> Result<MutationStart<'_>> {
        if op.0.is_nil() {
            return Err(Error::Invalid("operation_id is required".into()));
        }
        self.require_enabled(subject).await?;
        let mut result = MutationEnvelope::begin(
            &self.store,
            subject,
            op,
            kind,
            digest,
            Some(OwnerLock("self-authority")),
        )
        .await?;
        if let MutationStart::Active(m) = &mut result {
            self.runtime.capture_semantic_in(m.tx(), subject).await?;
        }
        Ok(result)
    }
    async fn validate_basis(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        validate_exact_basis(basis)?;
        let evidence = basis
            .iter()
            .filter_map(|b| {
                if let RevisionBasis::Evidence(e) = b {
                    Some(e.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        self.material
            .lock_occurrences_in(
                tx,
                subject,
                &evidence.iter().map(|e| e.occurrence_id).collect::<Vec<_>>(),
            )
            .await?;
        self.material
            .evidence_roots_batch_in(tx, subject, &evidence, None)
            .await?;
        for item in basis {
            match item {
                RevisionBasis::Evidence(_) => {}
                RevisionBasis::CognitionDependency(d) => {
                    self.store
                        .bind_exact_reference_in(tx, subject, &d.target_revision)
                        .await?;
                    if !self
                        .store
                        .cognition_available_in(tx, subject, &d.target_revision)
                        .await?
                    {
                        return Err(Error::Unavailable(
                            "Self source cognition is unavailable".into(),
                        ));
                    }
                }
                RevisionBasis::Seed(seed) => {
                    let hash:String=sqlx::query_scalar("SELECT content_hash FROM cognitive_seed_versions WHERE subject_id=$1 AND seed_version_id=$2 FOR SHARE").bind(subject.0).bind(seed.seed_version_id.0).fetch_optional(&mut **tx).await.map_err(db)?.ok_or_else(||Error::Invalid("Seed source is outside Subject".into()))?;
                    let text = String::from_utf8(self.objects.get(&hash).await?)
                        .map_err(|_| Error::Invalid("Seed is not UTF-8".into()))?;
                    validate_seed_locator(&text, &seed.semantic_path)?;
                }
            }
        }
        Ok(())
    }
    async fn validate_content(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        content: &SelfContent,
    ) -> Result<()> {
        content.validate()?;
        self.validate_basis(tx, subject, content.basis()).await?;
        match content {
            SelfContent::SelfFacet(v) => {
                for e in &v.exceptions {
                    self.validate_basis(tx, subject, &e.basis).await?;
                }
                if let Some(w) = v.scope.work_context
                    && !self
                        .store
                        .reference_in_subject_tx(tx, subject, &CognitiveRef::WorkContext(w))
                        .await?
                {
                    return Err(Error::Invalid(
                        "Self scope WorkContext is outside Subject".into(),
                    ));
                }
                for reference in v
                    .domain_knowledge
                    .iter()
                    .flat_map(|knowledge| &knowledge.interest_refs)
                {
                    self.store
                        .bind_exact_reference_in(tx, subject, reference)
                        .await?;
                    if !self
                        .store
                        .cognition_available_in(tx, subject, reference)
                        .await?
                    {
                        return Err(Error::Unavailable(
                            "knowledge interest is unavailable".into(),
                        ));
                    }
                }
                for e in &v.scope.entities {
                    if !self
                        .store
                        .reference_in_subject_tx(tx, subject, &CognitiveRef::Entity(e.clone()))
                        .await?
                    {
                        return Err(Error::Invalid(
                            "Self scope Entity is outside Subject".into(),
                        ));
                    }
                }
            }
            SelfContent::NarrativeThread(v) => {
                for r in &v.source_refs {
                    self.store.bind_exact_reference_in(tx, subject, r).await?;
                    if !self.store.cognition_available_in(tx, subject, r).await? {
                        return Err(Error::Unavailable(
                            "Narrative source cognition is unavailable".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

fn validate_seed_locator(text: &str, path: &str) -> Result<()> {
    if path == "$" {
        return Ok(());
    }
    let range = path
        .strip_prefix("line:")
        .ok_or_else(|| Error::Invalid("Seed locator must be $ or line:start-end".into()))?;
    let (a, b) = range.split_once('-').unwrap_or((range, range));
    let start = a
        .parse::<usize>()
        .map_err(|_| Error::Invalid("invalid Seed line".into()))?;
    let end = b
        .parse::<usize>()
        .map_err(|_| Error::Invalid("invalid Seed line".into()))?;
    if start == 0 || end < start || end > text.lines().count() {
        return Err(Error::Invalid(
            "Seed locator is outside preserved source".into(),
        ));
    }
    Ok(())
}
