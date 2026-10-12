//! Host identity binding of a source mention belongs to Material Authority.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_persistence::database_error as db;
impl MaterialService {
    pub async fn rebind_entity(
        &self,
        subject: SubjectId,
        mention: Uuid,
        entity: Option<EntityRef>,
        state: String,
        host_resolution_ref: Option<String>,
        reason: Option<String>,
    ) -> Result<()> {
        if !matches!(state.as_str(), "bound" | "unbound" | "disputed")
            || (state == "bound" && entity.is_none())
            || (state == "unbound" && entity.is_some())
            || host_resolution_ref.as_ref().is_some_and(|v| v.len() > 1024)
            || reason.as_ref().is_some_and(|v| v.len() > 4096)
        {
            return Err(Error::Invalid("invalid source identity binding".into()));
        }
        self.store.require_subject(subject).await?;
        let mut tx = self.store.begin().await?;
        let owned: Option<Uuid> = sqlx::query_scalar("SELECT mention_id FROM entity_mentions WHERE subject_id=$1 AND mention_id=$2 FOR UPDATE")
            .bind(subject.0).bind(mention).fetch_optional(&mut *tx).await.map_err(db)?;
        if owned.is_none() {
            return Err(Error::NotFound("source mention not found".into()));
        }
        if let Some(entity) = &entity
            && !self
                .store
                .reference_in_subject_tx(&mut tx, subject, &CognitiveRef::Entity(entity.clone()))
                .await?
        {
            return Err(Error::Invalid("Entity does not belong to Subject".into()));
        }
        let revision: i32 = sqlx::query_scalar("SELECT COALESCE(max(revision_no),0)+1 FROM entity_binding_revisions WHERE mention_id=$1")
            .bind(mention).fetch_one(&mut *tx).await.map_err(db)?;
        sqlx::query("INSERT INTO entity_binding_revisions(binding_revision_id,mention_id,revision_no,entity_ref,binding_state,host_resolution_ref,reason,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(Uuid::now_v7()).bind(mention).bind(revision).bind(entity.map(|e|e.as_str().to_owned())).bind(state).bind(host_resolution_ref).bind(reason).bind(self.cognition.now(subject)).execute(&mut *tx).await.map_err(db)?;
        AuthorityStore::invalidate_in(&mut tx, subject, ProjectionInvalidation::text()).await?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }
}

impl MaterialService {
    pub async fn resolved_mentions(
        &self,
        subject: SubjectId,
        occurrence: OccurrenceId,
    ) -> Result<Vec<(Uuid, String, EntityRef)>> {
        let rows = sqlx::query("SELECT m.mention_id,m.surface,b.entity_ref FROM entity_mentions m JOIN LATERAL (SELECT entity_ref,binding_state FROM entity_binding_revisions WHERE mention_id=m.mention_id ORDER BY revision_no DESC LIMIT 1) b ON b.binding_state='bound' AND b.entity_ref IS NOT NULL WHERE m.subject_id=$1 AND m.occurrence_id=$2 ORDER BY m.mention_id LIMIT 129")
            .bind(subject.0).bind(occurrence.0).fetch_all(self.store.pool()).await.map_err(db)?;
        if rows.len() > 128 {
            return Err(Error::Invalid(
                "resolved mention candidate bound exceeded".into(),
            ));
        }
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("mention_id").map_err(db)?,
                    row.try_get("surface").map_err(db)?,
                    EntityRef::new(row.try_get::<String, _>("entity_ref").map_err(db)?)?,
                ))
            })
            .collect()
    }
}
