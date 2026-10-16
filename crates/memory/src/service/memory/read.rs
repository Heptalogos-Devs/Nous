// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn revision(
        &self,
        subject: SubjectId,
        revision: MemoryRevisionId,
    ) -> Result<MemoryView> {
        let mut tx = self.store.read_view().await?;
        let result = self
            .full_memory_views_in(&mut tx, subject, (&[revision.0], &[]))
            .await?
            .remove(&revision.0)
            .ok_or_else(|| Error::NotFound("memory not found".into()))?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }

    pub async fn memory(
        &self,
        subject: SubjectId,
        memory_id: MemoryId,
        revision: Option<MemoryRevisionId>,
    ) -> Result<MemoryView> {
        let mut tx = self.store.read_view().await?;
        let revisions = revision.map(|id| vec![id.0]).unwrap_or_default();
        let objects = if revision.is_none() {
            vec![memory_id.0]
        } else {
            vec![]
        };
        let views = self
            .full_memory_views_in(&mut tx, subject, (&revisions, &objects))
            .await?;
        tx.commit().await.map_err(db)?;
        views
            .into_values()
            .find(|view| view.object.memory_id == memory_id)
            .ok_or_else(|| Error::NotFound("memory not found".into()))
    }

    pub async fn memories(
        &self,
        subject: SubjectId,
        status: &str,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<MemoryView>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("page limit is out of bounds".into()));
        }
        self.store.require_subject(subject).await?;
        if !matches!(status, "" | "accepted" | "withdrawn") {
            return Err(Error::Invalid("invalid memory acceptance status".into()));
        }
        let mut tx = self.store.read_view().await?;
        let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT current_revision_id FROM memory_objects WHERE subject_id=$1 AND ($2='' OR acceptance_state=$2) AND ($3::uuid IS NULL OR memory_id>$3) ORDER BY memory_id LIMIT $4")
            .bind(subject.0).bind(status).bind(after).bind(limit+1).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let mut views = self
            .full_memory_views_in(&mut tx, subject, (&ids, &[]))
            .await?;
        tx.commit().await.map_err(db)?;
        let items = ids
            .into_iter()
            .map(|id| {
                views
                    .remove(&id)
                    .ok_or_else(|| Error::NotFound("memory page changed while reading".into()))
            })
            .collect::<Result<_>>()?;
        Ok((items, more))
    }

    async fn full_memory_views_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        selection: (&[Uuid], &[Uuid]),
    ) -> Result<std::collections::HashMap<Uuid, MemoryView>> {
        let policy =
            resolve_accessibility_policy(&self.configuration.snapshot_for_subject(subject)?)?;
        self.memory_views_in(
            tx,
            subject,
            selection,
            &policy,
            None,
            self.cognition.recorded_now(),
            None,
        )
        .await
    }

    pub async fn memory_history(
        &self,
        subject: SubjectId,
        memory_id: MemoryId,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<MemoryView>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("page limit is out of bounds".into()));
        }
        let mut tx = self.store.read_view().await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM memory_objects WHERE subject_id=$1 AND memory_id=$2)",
        )
        .bind(subject.0)
        .bind(memory_id.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if !exists {
            return Err(Error::NotFound("memory not found".into()));
        }
        let after_no = if let Some(after) = after {
            Some(sqlx::query_scalar::<_, i32>("SELECT revision_no FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2 AND memory_revision_id=$3")
                .bind(subject.0).bind(memory_id.0).bind(after).fetch_optional(&mut *tx).await.map_err(db)?
                .ok_or_else(|| Error::Invalid("invalid memory history cursor".into()))?)
        } else {
            None
        };
        let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT memory_revision_id FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2 AND ($3::int IS NULL OR revision_no>$3) ORDER BY revision_no LIMIT $4")
            .bind(subject.0).bind(memory_id.0).bind(after_no).bind(limit+1).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let mut views = self
            .full_memory_views_in(&mut tx, subject, (&ids, &[]))
            .await?;
        tx.commit().await.map_err(db)?;
        let items = ids
            .into_iter()
            .map(|id| {
                views
                    .remove(&id)
                    .ok_or_else(|| Error::NotFound("memory history changed while reading".into()))
            })
            .collect::<Result<_>>()?;
        Ok((items, more))
    }
}
