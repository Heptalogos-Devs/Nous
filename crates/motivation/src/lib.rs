//! Motivation owns wishes, concerns and desired conditions, never Pursuit or effects.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod configuration;
mod context;
pub use configuration::*;
mod contract;
mod criterion_reader;
mod evaluator;
mod historical;
mod lifecycle;
mod mutations;
mod query;
mod read;
mod selection;
mod semantic;
mod source_read;
pub use criterion_reader::*;
pub use evaluator::*;
pub use selection::*;
pub use semantic::*;

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
pub struct MotivationService {
    store: AuthorityStore,
    material: MaterialService,
    objects: ObjectStore,
    runtime: CognitiveRuntimeService,
    clocks: nous_runtime::SubjectClockService,
    pub(crate) roots: std::sync::Arc<dyn nous_runtime::SourceRootResolver>,
}
impl MotivationService {
    pub fn new(
        store: AuthorityStore,
        material: MaterialService,
        objects: ObjectStore,
        runtime: CognitiveRuntimeService,
        clocks: nous_runtime::SubjectClockService,
        roots: std::sync::Arc<dyn nous_runtime::SourceRootResolver>,
    ) -> Self {
        Self {
            store,
            material,
            objects,
            runtime,
            clocks,
            roots,
        }
    }
    pub async fn require_enabled(&self, subject: SubjectId) -> Result<()> {
        let enabled: Option<bool> =
            sqlx::query_scalar("SELECT motivation FROM subject_capabilities WHERE subject_id=$1")
                .bind(subject.0)
                .fetch_optional(self.store.pool())
                .await
                .map_err(db)?;
        match enabled {
            Some(true) => Ok(()),
            Some(false) => Err(DomainError::new(
                DomainErrorCode::CapabilityUnavailable,
                "Subject Motivation capability is disabled",
            )
            .with_context("capability", "motivation")
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
            Some(OwnerLock("motivation-authority")),
        )
        .await?;
        if let MutationStart::Active(m) = &mut result {
            self.runtime.capture_semantic_in(m.tx(), subject).await?;
        }
        Ok(result)
    }
    pub(crate) async fn validate_basis(
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
                            "Motivation source cognition is unavailable".into(),
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
        content: &MotivationContent,
    ) -> Result<()> {
        content.validate()?;
        if let Some(condition) = &content.matter().condition {
            let cfg = self.runtime.configuration.snapshot_for_subject(subject)?;
            condition
                .expression
                .validate(cfg.get(CRITERION_DEPTH)?, cfg.get(CRITERION_LEAVES)?)?;
        }

        self.validate_basis(tx, subject, content.basis()).await?;
        let references = content
            .matter()
            .links
            .iter()
            .map(|l| l.target_revision.clone())
            .chain(
                content
                    .matter()
                    .condition
                    .iter()
                    .flat_map(|c| criterion_references(&c.expression)),
            )
            .collect::<Vec<_>>();
        for reference in references {
            self.store
                .bind_exact_reference_in(tx, subject, &reference)
                .await?;
        }
        for entity in content.entities() {
            if !self
                .store
                .reference_in_subject_tx(tx, subject, &CognitiveRef::Entity(entity))
                .await?
            {
                return Err(Error::Invalid(
                    "Motivation Entity is not bound by Host in this Subject".into(),
                ));
            }
        }
        if let Some(w) = content.scope().work_context
            && !self
                .store
                .reference_in_subject_tx(tx, subject, &CognitiveRef::WorkContext(w))
                .await?
        {
            return Err(Error::Invalid(
                "Motivation work context is outside Subject".into(),
            ));
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

pub fn criterion_references(c: &Criterion) -> Vec<CognitiveRef> {
    match c {
        Criterion::OwnerState { reference, .. } | Criterion::Relation { reference, .. } => {
            vec![reference.clone()]
        }
        Criterion::Semantic { material_refs, .. } => material_refs.clone(),
        Criterion::All { items } | Criterion::Any { items } => {
            items.iter().flat_map(criterion_references).collect()
        }
        Criterion::Not { item } => criterion_references(item),
        _ => vec![],
    }
}

/// Only owner-state selectors read lifecycle metadata; relation/semantic leaves read content.
pub(crate) fn criterion_accesses(c: &Criterion) -> Vec<(CognitiveRef, bool)> {
    match c {
        Criterion::OwnerState { reference, .. } => vec![(reference.clone(), true)],
        Criterion::Relation { reference, .. } => vec![(reference.clone(), false)],
        Criterion::Semantic { material_refs, .. } => {
            material_refs.iter().cloned().map(|r| (r, false)).collect()
        }
        Criterion::All { items } | Criterion::Any { items } => {
            items.iter().flat_map(criterion_accesses).collect()
        }
        Criterion::Not { item } => criterion_accesses(item),
        _ => vec![],
    }
}
