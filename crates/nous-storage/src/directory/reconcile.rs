use crate::{WriteTransaction, storage_error};
use nous_domain::{media::*, *};
use sqlx::Row;

impl WriteTransaction {
    /// Rebuild missing secondary addresses from authoritative identities, without
    /// revising cognition, source content, timestamps or any existing address.
    pub async fn reconcile_reference_addresses(
        &mut self,
        namespace: &NamespaceId,
    ) -> Result<usize> {
        self.context().require_namespace(namespace)?;
        self.namespace(namespace).await?;
        let mut targets = Vec::new();
        let memories=sqlx::query("SELECT memory_id,revision FROM memory_revisions WHERE namespace_id=? ORDER BY memory_id,revision")
            .bind(namespace.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in memories {
            targets.push(RetrievalTarget::Cognition(CognitiveRef::memory(
                &MemoryId(row.get("memory_id")),
                row.get("revision"),
            )));
        }
        let objects=sqlx::query("SELECT object_id,kind,revision FROM structured_revisions WHERE namespace_id=? ORDER BY object_id,revision")
            .bind(namespace.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in objects {
            targets.push(RetrievalTarget::Cognition(CognitiveRef {
                kind: crate::codec::parse_enum(row.get("kind"))?,
                id: row.get("object_id"),
                revision: row.get("revision"),
            }));
        }
        let originals: Vec<String> =
            sqlx::query_scalar("SELECT id FROM materials WHERE namespace_id=? ORDER BY id")
                .bind(namespace.as_str())
                .fetch_all(&mut *self.tx)
                .await
                .map_err(storage_error)?;
        targets.extend(originals.into_iter().map(|id| {
            RetrievalTarget::Media(MediaReference::Original {
                material_id: MaterialId(id),
            })
        }));
        let regions: Vec<String> =
            sqlx::query_scalar("SELECT id FROM source_regions WHERE namespace_id=? ORDER BY id")
                .bind(namespace.as_str())
                .fetch_all(&mut *self.tx)
                .await
                .map_err(storage_error)?;
        targets.extend(regions.into_iter().map(|id| {
            RetrievalTarget::Media(MediaReference::SourceRegion {
                id: SourceRegionId(id),
            })
        }));
        let representations=sqlx::query("SELECT id,revision FROM media_representations WHERE namespace_id=? ORDER BY id,revision")
            .bind(namespace.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in representations {
            targets.push(RetrievalTarget::Media(MediaReference::Representation {
                reference: RepresentationRef {
                    id: DerivedRepresentationId(row.get("id")),
                    revision: row.get("revision"),
                },
            }));
        }
        let regions: Vec<String> =
            sqlx::query_scalar("SELECT id FROM derived_regions WHERE namespace_id=? ORDER BY id")
                .bind(namespace.as_str())
                .fetch_all(&mut *self.tx)
                .await
                .map_err(storage_error)?;
        targets.extend(regions.into_iter().map(|id| {
            RetrievalTarget::Media(MediaReference::DerivedRegion {
                id: DerivedRegionId(id),
            })
        }));
        for target in &targets {
            self.ensure_retrieval_addresses(namespace, target).await?;
        }
        Ok(targets.len())
    }
}
