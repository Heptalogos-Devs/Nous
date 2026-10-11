//! Atomic Authority/Serving freshness stamps used by read-side coordinators.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error};
use nous_core::{Error, Result, SubjectId};
use sqlx::Row;

/// Subject chronology and Serving purge fence captured by one database read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityReadStamp {
    pub authority_seq: i64,
    pub serving_purge_epoch: i64,
}

impl AuthorityStore {
    /// Capture the Authority sequence and Serving purge fence atomically.
    pub async fn read_context_stamp(&self, subject: SubjectId) -> Result<AuthorityReadStamp> {
        let row = sqlx::query(
            "SELECT authority_seq,serving_purge_epoch FROM subjects WHERE subject_id=$1",
        )
        .bind(subject.0)
        .fetch_optional(self.pool())
        .await
        .map_err(database_error)?
        .ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Subject,
                nous_core::ErrorCode::NotFound,
                "subject not found",
            )
        })?;
        Ok(AuthorityReadStamp {
            authority_seq: row.try_get("authority_seq").map_err(database_error)?,
            serving_purge_epoch: row.try_get("serving_purge_epoch").map_err(database_error)?,
        })
    }

    pub async fn serving_purge_epoch(&self, subject: SubjectId) -> Result<i64> {
        sqlx::query_scalar::<_, i64>("SELECT serving_purge_epoch FROM subjects WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_optional(self.pool())
            .await
            .map_err(database_error)?
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Subject,
                    nous_core::ErrorCode::NotFound,
                    "subject not found",
                )
            })
    }
}
