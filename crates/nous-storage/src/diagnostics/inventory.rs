use crate::{ReadView, storage_error};
use nous_domain::{media::*, *};
use sqlx::Row;
use std::collections::BTreeSet;

impl ReadView {
    pub async fn subject_inventory(
        &mut self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<SubjectInventory> {
        let scope = self.source_read_view(ctx, namespace, subject).await?;
        let mut inventory = SubjectInventory::default();
        let current =
            crate::vector::query_references(&mut self.tx, namespace, subject, None, false).await?;
        for reference in current {
            let target = RetrievalTarget::Cognition(reference.clone());
            if !self.source_read_allowed(&scope, &target).await? {
                continue;
            }
            let (active, deleted) = if reference.kind == CognitiveKind::Memory {
                let value = crate::cognition::memory::memory(
                    &mut self.tx,
                    namespace,
                    &MemoryId(reference.id.clone()),
                )
                .await?;
                (value.is_active(), value.deleted)
            } else {
                let value = crate::cognition::structured::read::object(
                    &mut self.tx,
                    namespace,
                    &CognitiveId(reference.id.clone()),
                    Some(reference.revision),
                )
                .await?;
                (value.is_active(), value.deleted)
            };
            let available = self
                .bundle_target_active(namespace, subject, &target)
                .await?;
            let counts = inventory
                .cognition
                .entry(crate::codec::enum_name(&reference.kind)?)
                .or_default();
            counts.current += 1;
            if !available {
                counts.withdrawn += 1;
            }
            if deleted {
                counts.deleted += 1;
            } else if active && available {
                counts.active += 1;
            } else {
                counts.inactive += 1;
            }
        }
        for reference in
            crate::vector::query_references(&mut self.tx, namespace, subject, None, true).await?
        {
            if self
                .source_read_allowed(&scope, &RetrievalTarget::Cognition(reference.clone()))
                .await?
            {
                inventory
                    .cognition
                    .entry(crate::codec::enum_name(&reference.kind)?)
                    .or_default()
                    .revisions += 1;
            }
        }
        let rows = sqlx::query(
            "SELECT id FROM observations WHERE namespace_id=? AND subject_id=? ORDER BY id",
        )
        .bind(namespace.as_str())
        .bind(subject.as_str())
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        for row in rows {
            let event = self
                .observation(namespace, &ObservationId(row.get("id")))
                .await?;
            match self
                .require_observation_read(ctx, namespace, subject, &event.id)
                .await
            {
                Ok(()) => {
                    *inventory
                        .observations
                        .entry(crate::codec::enum_name(&event.kind)?)
                        .or_default() += 1
                }
                Err(error) if hidden(&error) => {}
                Err(error) => return Err(error),
            }
        }
        let ids = self.subject_material_ids(namespace, subject).await?;
        let mut originals = BTreeSet::new();
        for id in ids {
            if self
                .source_read_allowed(
                    &scope,
                    &RetrievalTarget::Media(MediaReference::Original {
                        material_id: id.clone(),
                    }),
                )
                .await?
            {
                originals.insert(id);
            }
        }
        inventory
            .media
            .insert("original".into(), originals.len() as u64);
        let rows = sqlx::query("SELECT body FROM source_regions WHERE namespace_id=? ORDER BY id")
            .bind(namespace.as_str())
            .fetch_all(&mut *self.tx)
            .await
            .map_err(storage_error)?;
        for row in rows {
            let region: SourceRegion = serde_json::from_str(row.get("body"))?;
            if originals.contains(&region.material_id)
                && self
                    .source_read_allowed(
                        &scope,
                        &RetrievalTarget::Media(MediaReference::SourceRegion { id: region.id }),
                    )
                    .await?
            {
                *inventory.media.entry("source_region".into()).or_default() += 1;
            }
        }
        for (name, sql) in [
            (
                "representation",
                "SELECT body FROM media_representations r WHERE namespace_id=? AND subject_id=? AND revision=(SELECT max(h.revision) FROM media_representations h WHERE h.id=r.id) ORDER BY id",
            ),
            (
                "derived_region",
                "SELECT body FROM derived_regions WHERE namespace_id=? AND subject_id=? ORDER BY id",
            ),
        ] {
            let rows = sqlx::query(sql)
                .bind(namespace.as_str())
                .bind(subject.as_str())
                .fetch_all(&mut *self.tx)
                .await
                .map_err(storage_error)?;
            for row in rows {
                let reference = if name == "representation" {
                    MediaReference::Representation {
                        reference: serde_json::from_str::<DerivedRepresentation>(row.get("body"))?
                            .reference(),
                    }
                } else {
                    MediaReference::DerivedRegion {
                        id: serde_json::from_str::<DerivedRegion>(row.get("body"))?.id,
                    }
                };
                if self
                    .source_read_allowed(&scope, &RetrievalTarget::Media(reference))
                    .await?
                {
                    *inventory.media.entry(name.into()).or_default() += 1;
                }
            }
        }
        let rows = sqlx::query(
            "SELECT memory_id,revision FROM memory_purges WHERE namespace_id=? AND subject_id=?",
        )
        .bind(namespace.as_str())
        .bind(subject.as_str())
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        for row in rows {
            let reference = CognitiveRef::memory(
                &MemoryId(row.get("memory_id")),
                row.get::<i64, _>("revision") - 1,
            );
            match self
                .require_source_provenance(
                    ctx,
                    namespace,
                    subject,
                    &[SourceReadRef::Cognition { reference }],
                )
                .await
            {
                Ok(()) => inventory.purged_memories += 1,
                Err(error) if hidden(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(inventory)
    }
}
pub(super) fn hidden(error: &Error) -> bool {
    matches!(error.code, ErrorCode::Forbidden | ErrorCode::NotFound)
}
