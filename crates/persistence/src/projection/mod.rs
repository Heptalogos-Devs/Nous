//! Explicit per-family Authority watermarks for independent serving updates.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

pub(crate) mod concept;
pub(crate) mod longitudinal;
pub(crate) mod lookup;
pub(crate) mod text;
pub(crate) mod topology;

use crate::{AuthorityStore, database_error as db};
use nous_core::{Result, SubjectId};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum DenseInvalidation {
    #[default]
    None,
    All,
    Spaces(Vec<String>),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectionInvalidation {
    pub lexical: bool,
    pub dense: DenseInvalidation,
    pub topology: bool,
    pub synopsis: bool,
}

impl ProjectionInvalidation {
    pub fn text() -> Self {
        Self {
            lexical: true,
            dense: DenseInvalidation::All,
            ..Self::default()
        }
    }

    pub fn topology() -> Self {
        Self {
            topology: true,
            ..Self::default()
        }
    }

    pub fn identity() -> Self {
        Self {
            topology: true,
            ..Self::default()
        }
    }

    pub fn all() -> Self {
        Self {
            lexical: true,
            dense: DenseInvalidation::All,
            topology: true,
            synopsis: true,
        }
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.lexical |= other.lexical;
        self.topology |= other.topology;
        self.synopsis |= other.synopsis;
        self.dense = match (std::mem::take(&mut self.dense), other.dense) {
            (DenseInvalidation::All, _) | (_, DenseInvalidation::All) => DenseInvalidation::All,
            (DenseInvalidation::None, right) => right,
            (left, DenseInvalidation::None) => left,
            (DenseInvalidation::Spaces(mut left), DenseInvalidation::Spaces(right)) => {
                left.extend(right);
                left.sort();
                left.dedup();
                DenseInvalidation::Spaces(left)
            }
        };
    }

    fn families(&self) -> Vec<(&'static str, String)> {
        let mut result = Vec::new();
        for (family, changed) in [
            ("lexical", self.lexical),
            ("topology", self.topology),
            ("synopsis", self.synopsis),
        ] {
            if changed {
                result.push((family, String::new()));
            }
        }
        // Concept postings follow current revision eligibility as well as Tag/Association changes.
        if self.topology || self.lexical {
            result.push(("concept", "*".into()));
        }
        match &self.dense {
            DenseInvalidation::None => {}
            DenseInvalidation::All => result.push(("dense", "*".into())),
            DenseInvalidation::Spaces(spaces) => {
                result.extend(spaces.iter().cloned().map(|space| ("dense", space)))
            }
        }
        result
    }
}

impl AuthorityStore {
    pub async fn invalidate(
        &self,
        subject: SubjectId,
        changes: ProjectionInvalidation,
    ) -> Result<i64> {
        let mut tx = self.begin().await?;
        let revision = Self::invalidate_in(&mut tx, subject, changes).await?;
        tx.commit().await.map_err(db)?;
        Ok(revision)
    }

    pub async fn invalidate_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        changes: ProjectionInvalidation,
    ) -> Result<i64> {
        let revision = sqlx::query_scalar::<_, i64>("UPDATE subjects SET authority_seq=authority_seq+1 WHERE subject_id=$1 RETURNING authority_seq")
            .bind(subject.0).fetch_one(&mut **tx).await.map_err(db)?;
        Self::mark_projection_families_in(tx, subject, revision, changes).await?;
        Ok(revision)
    }

    /// Extend the affected families of the same Authority transaction.
    pub async fn mark_projection_families_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        revision: i64,
        changes: ProjectionInvalidation,
    ) -> Result<()> {
        for (family, space) in changes.families() {
            sqlx::query("INSERT INTO projection_watermarks(subject_id,family,space_signature,desired_authority_seq) VALUES($1,$2,$3,$4) ON CONFLICT(subject_id,family,space_signature) DO UPDATE SET desired_authority_seq=GREATEST(projection_watermarks.desired_authority_seq,excluded.desired_authority_seq)")
                .bind(subject.0).bind(family).bind(space).bind(revision)
                .execute(&mut **tx).await.map_err(db)?;
        }
        Ok(())
    }
}
