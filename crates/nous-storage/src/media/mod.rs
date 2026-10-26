//! Exact media locators and immutable derivation graphs inside Authority snapshots.
mod graph;
mod read;
mod references;
mod workflow;
mod write;
use crate::{Database, ReadView, WriteTransaction};
use nous_domain::{media::*, *};
use std::collections::BTreeSet;

pub(crate) async fn roots_in(
    connection: &mut sqlx::SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &MediaReference,
) -> Result<BTreeSet<MaterialId>> {
    graph::roots(connection, namespace, subject, reference).await
}

macro_rules! snapshot_reads {
    ($owner:ty) => {
        impl $owner {
            pub async fn source_region(
                &mut self,
                namespace: &NamespaceId,
                id: &SourceRegionId,
            ) -> Result<SourceRegion> {
                read::source_region(&mut self.tx, namespace, id).await
            }
            pub async fn representation(
                &mut self,
                namespace: &NamespaceId,
                subject: &SubjectId,
                reference: &RepresentationRef,
            ) -> Result<DerivedRepresentation> {
                read::representation(&mut self.tx, namespace, subject, reference).await
            }
            pub async fn derived_region(
                &mut self,
                namespace: &NamespaceId,
                subject: &SubjectId,
                id: &DerivedRegionId,
            ) -> Result<DerivedRegion> {
                read::derived_region(&mut self.tx, namespace, subject, id).await
            }
            pub async fn media_roots(
                &mut self,
                namespace: &NamespaceId,
                subject: &SubjectId,
                reference: &MediaReference,
            ) -> Result<BTreeSet<MaterialId>> {
                graph::roots(&mut self.tx, namespace, subject, reference).await
            }
            pub async fn media_duration(
                &mut self,
                namespace: &NamespaceId,
                subject: &SubjectId,
                reference: &MediaReference,
            ) -> Result<Option<u64>> {
                graph::duration(&mut self.tx, namespace, subject, reference).await
            }
            pub async fn representation_head(
                &mut self,
                namespace: &NamespaceId,
                subject: &SubjectId,
                id: &DerivedRepresentationId,
            ) -> Result<Option<DerivedRepresentation>> {
                read::head(&mut self.tx, namespace, subject, id).await
            }
            pub async fn derived_regions(
                &mut self,
                namespace: &NamespaceId,
                subject: &SubjectId,
                reference: &RepresentationRef,
            ) -> Result<Vec<DerivedRegion>> {
                read::regions(&mut self.tx, namespace, subject, reference).await
            }
        }
    };
}
snapshot_reads!(ReadView);
snapshot_reads!(WriteTransaction);
impl ReadView {
    pub async fn media_lineage(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
    ) -> Result<MediaSourceLineage> {
        let watermark = self.watermark;
        graph::lineage(&mut self.tx, namespace, subject, reference, watermark).await
    }
}
impl Database {
    pub async fn source_region(
        &self,
        namespace: &NamespaceId,
        id: &SourceRegionId,
    ) -> Result<SourceRegion> {
        let mut view = self.read_view().await?;
        let value = view.source_region(namespace, id).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn representation(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &RepresentationRef,
    ) -> Result<DerivedRepresentation> {
        let mut view = self.read_view().await?;
        let value = view.representation(namespace, subject, reference).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn derived_region(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        id: &DerivedRegionId,
    ) -> Result<DerivedRegion> {
        let mut view = self.read_view().await?;
        let value = view.derived_region(namespace, subject, id).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn media_roots(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
    ) -> Result<BTreeSet<MaterialId>> {
        let mut view = self.read_view().await?;
        let value = view.media_roots(namespace, subject, reference).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn derived_regions(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &RepresentationRef,
    ) -> Result<Vec<DerivedRegion>> {
        let mut view = self.read_view().await?;
        let value = view.derived_regions(namespace, subject, reference).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn representation_history(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        id: &DerivedRepresentationId,
        limit: usize,
    ) -> Result<Vec<DerivedRepresentation>> {
        let mut view = self.read_view().await?;
        let value = read::history(&mut view.tx, namespace, subject, id, limit).await?;
        view.finish().await?;
        Ok(value)
    }
}
