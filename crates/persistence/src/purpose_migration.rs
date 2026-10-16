// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
//! Finishes the one-time 0025 representation migration using the canonical owner encoder.
use crate::{AuthorityStore, database_error as db};
use nous_core::{Error, Result, SubjectId, canonical_request_digest};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

impl AuthorityStore {
    pub(crate) async fn finish_purpose_migration(&self) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("SELECT pg_advisory_xact_lock(250025)")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let pending: bool =
            sqlx::query_scalar("SELECT to_regclass('purpose_migration_receipts') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        if !pending {
            return Ok(());
        }
        let rows = sqlx::query("SELECT * FROM purpose_migration_receipts")
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
        for row in rows {
            let subject = SubjectId(row.try_get("subject_id").map_err(db)?);
            let operation: Uuid = row.try_get("operation_id").map_err(db)?;
            let kind: String = row.try_get("operation_kind").map_err(db)?;
            let old_digest: String = row.try_get("request_digest").map_err(db)?;
            let mut content: Value = row.try_get("old_content").map_err(db)?;
            // SelfContent serialization omits this real-domain default.
            if content["value"]["valid_time_domain_ref"] == "time:real" {
                content["value"]
                    .as_object_mut()
                    .expect("typed Self content")
                    .remove("valid_time_domain_ref");
            }
            let mut reasons: Vec<String> = row
                .try_get::<Vec<Option<String>>, _>("paid_reasons")
                .map_err(db)?
                .into_iter()
                .flatten()
                .collect();
            reasons.push(row.try_get("reason").map_err(db)?);
            let epoch: i64 = row.try_get("result_epoch").map_err(db)?;
            let revision: Uuid = row.try_get("revision_id").map_err(db)?;
            let parent: Option<Uuid> = row.try_get("parent_revision_id").map_err(db)?;
            let object: Uuid = row.try_get("object_id").map_err(db)?;
            let digest = replacement_digest(
                subject,
                operation,
                &kind,
                &old_digest,
                &content,
                &reasons,
                object,
                revision,
                parent,
                epoch,
            )?;
            sqlx::query("UPDATE mutation_receipts SET request_digest=$3 WHERE subject_id=$1 AND operation_id=$2 AND request_digest=$4")
                .bind(subject.0).bind(operation).bind(digest).bind(old_digest)
                .execute(&mut *tx).await.map_err(db)?;
        }
        sqlx::query("DROP TABLE purpose_migration_receipts")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "exact historical receipt components, confined to one-time migration"
)]
fn replacement_digest(
    subject: SubjectId,
    operation: Uuid,
    kind: &str,
    old_digest: &str,
    content: &Value,
    reasons: &[String],
    object: Uuid,
    revision: Uuid,
    parent: Option<Uuid>,
    epoch: i64,
) -> Result<String> {
    for reason in reasons {
        let base =
            json!({"subject":subject,"operation_id":operation,"content":content,"reason":reason});
        for mut request in request_candidates(base, kind, object, revision, parent, epoch) {
            if canonical_request_digest(kind, subject, &request)? != old_digest {
                continue;
            }
            formalize_request(&mut request);
            return canonical_request_digest(kind, subject, &request);
        }
    }
    Err(Error::Infrastructure(format!(
        "purpose migration cannot prove exact Self receipt {operation}"
    )))
}
fn request_candidates(
    base: Value,
    kind: &str,
    object: Uuid,
    revision: Uuid,
    parent: Option<Uuid>,
    epoch: i64,
) -> Vec<Value> {
    if kind == "self.adopt" {
        return vec![base];
    }
    let mut candidates = vec![];
    for head in [Some(revision), parent].into_iter().flatten() {
        for expected in [epoch, epoch - 1].into_iter().filter(|e| *e > 0) {
            let mut request = base.clone();
            request["object_id"] = json!(object);
            request["expected_revision"] = json!(head);
            request["expected_epoch"] = json!(expected);
            candidates.push(request);
        }
    }
    candidates
}
fn formalize_request(request: &mut Value) {
    let Some(purposes) = request
        .pointer_mut("/content/value/scope/purposes")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for purpose in purposes {
        let formal = match purpose.as_str() {
            Some("deliberation") => "subject_deliberation",
            Some("expression") => "subject_rendering",
            Some("worker") => "professional_execution",
            _ => continue,
        };
        *purpose = json!(formal);
    }
}
