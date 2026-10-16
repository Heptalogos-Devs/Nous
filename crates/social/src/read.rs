// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl SocialService {
    pub async fn get(&self, subject: SubjectId, id: Uuid, exact: bool) -> Result<SocialRecord> {
        self.require_enabled(subject).await?;
        let mut tx = self.store.read_view().await?;
        let record = self.read_in(&mut tx, subject, id, exact).await?;
        tx.commit().await.map_err(db)?;
        Ok(record)
    }
    pub async fn list(
        &self,
        subject: SubjectId,
        family: &str,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<(Vec<SocialRecord>, bool)> {
        self.require_enabled(subject).await?;
        if limit == 0
            || limit > 100
            || !["", "social_relation", "language_convention"].contains(&family)
        {
            return Err(Error::Invalid("invalid Social page or family".into()));
        }
        let mut tx = self.store.read_view().await?;
        let ids:Vec<Uuid>=sqlx::query_scalar("SELECT object_id FROM social_objects WHERE subject_id=$1 AND ($2='' OR family=$2) AND ($3::uuid IS NULL OR object_id>$3) ORDER BY object_id LIMIT $4").bind(subject.0).bind(family).bind(after).bind((limit+1) as i64).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit;
        let result = self
            .read_many_in(&mut tx, subject, &ids[..ids.len().min(limit)], false)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok((result, more))
    }
    pub async fn history(
        &self,
        subject: SubjectId,
        object: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<(Vec<SocialRecord>, bool)> {
        self.require_enabled(subject).await?;
        if limit == 0 || limit > 100 {
            return Err(Error::Invalid("invalid Social history page".into()));
        }
        let mut tx = self.store.read_view().await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_objects WHERE subject_id=$1 AND object_id=$2)",
        )
        .bind(subject.0)
        .bind(object)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if !exists {
            return Err(Error::NotFound(
                "Social object is outside Subject or purged".into(),
            ));
        }
        let ids:Vec<Uuid>=sqlx::query_scalar("SELECT revision_id FROM social_revisions WHERE subject_id=$1 AND object_id=$2 AND ($3::uuid IS NULL OR revision_id>$3) ORDER BY revision_id LIMIT $4").bind(subject.0).bind(object).bind(after).bind((limit+1) as i64).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit;
        let result = self
            .read_many_in(&mut tx, subject, &ids[..ids.len().min(limit)], true)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok((result, more))
    }
    pub(crate) async fn read_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        id: Uuid,
        exact: bool,
    ) -> Result<SocialRecord> {
        self.read_many_in(tx, subject, &[id], exact)
            .await?
            .pop()
            .ok_or_else(|| {
                Error::NotFound("Social object/revision is outside Subject or purged".into())
            })
    }
    /// Body and support are loaded in two bounded queries in the caller's read view.
    pub(crate) async fn read_many_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        ids: &[Uuid],
        exact: bool,
    ) -> Result<Vec<SocialRecord>> {
        use std::collections::HashMap;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows=sqlx::query("SELECT o.*,r.revision_id,r.revision_no,r.parent_revision_id,r.content,r.reason,r.formed_at,r.recorded_at FROM social_objects o JOIN social_revisions r ON r.object_id=o.object_id WHERE o.subject_id=$1 AND (($3 AND r.revision_id=ANY($2::uuid[])) OR (NOT $3 AND o.object_id=ANY($2::uuid[]) AND r.revision_id=o.current_revision_id))").bind(subject.0).bind(ids).bind(exact).fetch_all(&mut **tx).await.map_err(db)?;
        let revisions = rows
            .iter()
            .map(|r| r.try_get("revision_id").map_err(db))
            .collect::<Result<Vec<Uuid>>>()?;
        let support=sqlx::query("SELECT revision_id,role,basis,recorded_at FROM social_evidence WHERE revision_id=ANY($1::uuid[]) ORDER BY revision_id,recorded_at,role,basis_key").bind(&revisions).fetch_all(&mut **tx).await.map_err(db)?;
        let mut evidence = HashMap::<Uuid, Vec<SocialEvidence>>::new();
        for row in support {
            evidence
                .entry(row.try_get("revision_id").map_err(db)?)
                .or_default()
                .push(SocialEvidence {
                    role: row.try_get("role").map_err(db)?,
                    basis: serde_json::from_value(row.try_get("basis").map_err(db)?)
                        .map_err(|e| Error::Infrastructure(e.to_string()))?,
                    recorded_at: row.try_get("recorded_at").map_err(db)?,
                });
        }
        let mut records = HashMap::new();
        for row in rows {
            let revision_id = row.try_get("revision_id").map_err(db)?;
            let record = SocialRecord {
                object_id: row.try_get("object_id").map_err(db)?,
                subject_id: subject,
                revision_id,
                current_revision_id: row.try_get("current_revision_id").map_err(db)?,
                object_epoch: row.try_get("object_epoch").map_err(db)?,
                revision_no: row.try_get("revision_no").map_err(db)?,
                parent_revision_id: row.try_get("parent_revision_id").map_err(db)?,
                reason: row.try_get("reason").map_err(db)?,
                content: serde_json::from_value(row.try_get("content").map_err(db)?)
                    .map_err(|e| Error::Infrastructure(e.to_string()))?,
                acceptance_state: row.try_get("acceptance_state").map_err(db)?,
                integrity_state: row.try_get("integrity_state").map_err(db)?,
                suppression_state: row.try_get("suppression_state").map_err(db)?,
                created_at: row.try_get("created_at").map_err(db)?,
                formed_at: row.try_get("formed_at").map_err(db)?,
                recorded_at: row.try_get("recorded_at").map_err(db)?,
                evidence: evidence.remove(&revision_id).unwrap_or_default(),
            };
            records.insert(
                if exact {
                    record.revision_id
                } else {
                    record.object_id
                },
                record,
            );
        }
        Ok(ids.iter().filter_map(|id| records.remove(id)).collect())
    }
}
