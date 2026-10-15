// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl SelfService {
    pub async fn get(&self, subject: SubjectId, id: Uuid, exact: bool) -> Result<SelfRecord> {
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
    ) -> Result<(Vec<SelfRecord>, bool)> {
        self.require_enabled(subject).await?;
        if limit == 0 || limit > 100 || !["", "self_facet", "narrative_thread"].contains(&family) {
            return Err(Error::Invalid("invalid Self page or family".into()));
        }
        let mut tx = self.store.read_view().await?;
        let ids:Vec<Uuid>=sqlx::query_scalar("SELECT object_id FROM self_objects WHERE subject_id=$1 AND ($2='' OR family=$2) AND ($3::uuid IS NULL OR object_id>$3) ORDER BY object_id LIMIT $4").bind(subject.0).bind(family).bind(after).bind((limit+1) as i64).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit;
        let mut result = Vec::new();
        for id in ids.into_iter().take(limit) {
            result.push(self.read_in(&mut tx, subject, id, false).await?);
        }
        tx.commit().await.map_err(db)?;
        Ok((result, more))
    }
    pub async fn history(
        &self,
        subject: SubjectId,
        object: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<(Vec<SelfRecord>, bool)> {
        self.require_enabled(subject).await?;
        if limit == 0 || limit > 100 {
            return Err(Error::Invalid("invalid Self history page".into()));
        }
        let mut tx = self.store.read_view().await?;
        self.read_in(&mut tx, subject, object, false).await?;
        let ids:Vec<Uuid>=sqlx::query_scalar("SELECT revision_id FROM self_revisions WHERE subject_id=$1 AND object_id=$2 AND ($3::uuid IS NULL OR revision_id>$3) ORDER BY revision_id LIMIT $4").bind(subject.0).bind(object).bind(after).bind((limit+1) as i64).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit;
        let mut result = Vec::new();
        for id in ids.into_iter().take(limit) {
            result.push(self.read_in(&mut tx, subject, id, true).await?);
        }
        tx.commit().await.map_err(db)?;
        Ok((result, more))
    }
    pub(crate) async fn read_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        id: Uuid,
        exact: bool,
    ) -> Result<SelfRecord> {
        let row=sqlx::query("SELECT o.*,r.revision_id,r.revision_no,r.parent_revision_id,r.content,r.reason,r.formed_at,r.recorded_at FROM self_objects o JOIN self_revisions r ON r.object_id=o.object_id WHERE o.subject_id=$1 AND (($3 AND r.revision_id=$2) OR (NOT $3 AND o.object_id=$2 AND r.revision_id=o.current_revision_id))").bind(subject.0).bind(id).bind(exact).fetch_optional(&mut **tx).await.map_err(db)?.ok_or_else(||Error::NotFound("Self object/revision is outside Subject or purged".into()))?;
        let revision_id = row.try_get("revision_id").map_err(db)?;
        let rows=sqlx::query("SELECT role,basis,recorded_at FROM self_evidence WHERE revision_id=$1 ORDER BY recorded_at,role,basis_key").bind(revision_id).fetch_all(&mut **tx).await.map_err(db)?;
        let evidence = rows
            .into_iter()
            .map(|r| {
                Ok(SelfEvidence {
                    role: r.try_get("role").map_err(db)?,
                    basis: serde_json::from_value(r.try_get("basis").map_err(db)?)
                        .map_err(|e| Error::Infrastructure(e.to_string()))?,
                    recorded_at: r.try_get("recorded_at").map_err(db)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(SelfRecord {
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
            evidence,
        })
    }
}
