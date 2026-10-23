// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see NOTICE.
use super::validation::*;
use crate::codec::*;
use crate::source::references::*;
use crate::{WriteTransaction, write_error};
use nous_domain::*;
use sqlx::SqliteConnection;
use std::collections::HashSet;

impl WriteTransaction {
    /// One revision owner, exact typed inputs, and a scoped compare-and-swap.
    pub async fn save_structured(
        &mut self,
        value: &StructuredObject,
        expected_revision: Option<i64>,
    ) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        value.validate()?;
        let revision = expected_revision
            .unwrap_or(0)
            .checked_add(1)
            .filter(|v| *v > 0)
            .ok_or_else(|| Error::invalid("structured revision is out of range"))?;
        if value.revision != revision {
            return Err(Error::invalid(
                "structured revision must follow expected_revision",
            ));
        }
        self.subject(&value.namespace_id, &value.subject_id).await?;
        let previous = if let Some(expected) = expected_revision {
            let old = self
                .structured_object(&value.namespace_id, &value.id)
                .await?;
            if old.revision != expected
                || old.subject_id != value.subject_id
                || old.kind() != value.kind()
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "structured identity, scope or revision changed",
                ));
            }
            Some(old)
        } else {
            None
        };
        let mut body = value.clone();
        body.evidence.clear();
        let body_references = body.references();
        let previous_sources = previous
            .as_ref()
            .map(|old| {
                old.sources()
                    .into_iter()
                    .map(|source| source_parts(&source))
                    .collect::<Result<HashSet<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        for target in value.references() {
            if let Err(error) = validate_reference(
                &mut self.tx,
                &value.namespace_id,
                &value.subject_id,
                &target,
            )
            .await
            {
                let retained =
                    previous_sources.contains(&source_parts(&source_reference(&target))?);
                if error.code != ErrorCode::NotFound || !retained {
                    return Err(error);
                }
            }
        }
        let body_evidence = body.body.evidence_links();
        for source in value.sources() {
            if let Err(error) = validate_source(
                &mut self.tx,
                &value.namespace_id,
                &value.subject_id,
                &source,
            )
            .await
            {
                let retained = previous_sources.contains(&source_parts(&source)?);
                if error.code != ErrorCode::NotFound || !retained {
                    return Err(error);
                }
            }
        }
        reject_cycle(&mut self.tx, value).await?;
        validate_content_in(&mut self.tx, value, previous.as_ref()).await?;
        if let Some(call_id) = &value.producer_call_id {
            let call = self.model_call(call_id).await?;
            if call.namespace_id != value.namespace_id
                || call.subject_id != value.subject_id
                || call.state != nous_domain::ModelCallState::Succeeded
            {
                return Err(Error::invalid(
                    "structured producer requires a successful scoped model call",
                ));
            }
        }
        let kind = enum_name(&value.kind())?;
        match expected_revision {
            None => {
                sqlx::query("INSERT INTO structured_objects(id,namespace_id,subject_id,kind,current_revision,commit_seq) VALUES (?,?,?,?,?,?)")
                .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&kind).bind(value.revision).bind(value.commit_seq)
                .execute(&mut *self.tx).await.map_err(write_error)?;
            }
            Some(expected) => {
                let changed=sqlx::query("UPDATE structured_objects SET current_revision=?,commit_seq=? WHERE id=? AND namespace_id=? AND subject_id=? AND kind=? AND current_revision=?")
                .bind(value.revision).bind(value.commit_seq).bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&kind).bind(expected)
                .execute(&mut *self.tx).await.map_err(write_error)?.rows_affected();
                if changed != 1 {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "structured revision changed",
                    ));
                }
            }
        }
        sqlx::query("INSERT INTO structured_revisions(object_id,namespace_id,subject_id,revision,kind,body,times,producer_call_id,recorded_at,deleted,commit_seq) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.revision).bind(&kind).bind(serde_json::to_string(&value.body)?)
            .bind(serde_json::to_string(&value.times)?).bind(value.producer_call_id.as_ref().map(|id|id.as_str())).bind(&value.recorded_at).bind(value.deleted).bind(value.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        for (ordinal, target) in body_references.iter().enumerate() {
            let source = source_reference(target);
            save_link(&mut self.tx, value, ordinal, "body", &source, None).await?;
        }
        for (ordinal, link) in value.evidence.iter().enumerate() {
            save_link(
                &mut self.tx,
                value,
                ordinal,
                "evidence",
                &link.source,
                Some(link),
            )
            .await?;
        }
        for (ordinal, link) in body_evidence.iter().enumerate() {
            save_link(
                &mut self.tx,
                value,
                ordinal,
                "body_evidence",
                &link.source,
                Some(link),
            )
            .await?;
        }
        sync_aliases(&mut self.tx, value).await?;
        if value.deleted {
            self.retire_cognition_embedding_jobs(value).await?;
        }
        Ok(())
    }
}

async fn save_link(
    conn: &mut SqliteConnection,
    owner: &StructuredObject,
    ordinal: usize,
    category: &str,
    source: &EvidenceSource,
    link: Option<&EvidenceLink>,
) -> Result<()> {
    let (kind, id, revision) = source_parts(source)?;
    sqlx::query("INSERT INTO structured_links(namespace_id,subject_id,object_id,revision,ordinal,category,source_kind,source_id,source_revision,polarity,relation,explanation) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(owner.namespace_id.as_str()).bind(owner.subject_id.as_str()).bind(owner.id.as_str()).bind(owner.revision).bind(ordinal as i64).bind(category)
        .bind(kind).bind(id).bind(revision).bind(link.map(|l|enum_name(&l.polarity)).transpose()?).bind(link.and_then(|l|l.relation.as_ref()).map(enum_name).transpose()?)
        .bind(link.map(|l|l.explanation.as_str())).execute(conn).await.map_err(write_error)?;
    Ok(())
}
