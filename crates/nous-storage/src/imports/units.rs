use crate::{Database, ReadView, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::{media::SourceCoordinate, *};
use std::collections::BTreeSet;

impl WriteTransaction {
    pub async fn insert_import_unit_catalog(&mut self, value: &ImportUnitCatalog) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        value.id.validate()?;
        self.context()
            .require_subject(&value.namespace_id, &value.subject_id)?;
        if value.original_unit_count == 0
            || value.original_unit_count > 1000
            || value.documents.is_empty()
        {
            return Err(Error::invalid(
                "unit catalog requires 1 to 1000 original units and admitted inputs",
            ));
        }
        let document_count = value.documents.iter().try_fold(0usize, |count, document| {
            count
                .checked_add(document.original_unit_count)
                .ok_or_else(|| Error::invalid("catalog document unit count overflow"))
        })?;
        if document_count != value.original_unit_count {
            return Err(Error::invalid(
                "catalog document unit counts differ from original total",
            ));
        }
        let mut inputs = BTreeSet::new();
        for document in &value.documents {
            self.import_input_source(
                &value.namespace_id,
                &value.subject_id,
                &document.input_source_id,
            )
            .await?;
            if !inputs.insert(document.input_source_id.clone())
                || document.selected_strategy == ImportTextStrategy::Auto
            {
                return Err(Error::invalid(
                    "unit catalog has repeated inputs or an unresolved strategy",
                ));
            }
        }
        if let Some(parent) = &value.parent_catalog_id {
            if parent == &value.id {
                return Err(Error::invalid("unit catalog cannot be its own parent"));
            }
            read_catalog(&mut self.tx, &value.namespace_id, &value.subject_id, parent).await?;
        }
        sqlx::query("INSERT INTO import_unit_catalogs(id,body) VALUES(?,?)")
            .bind(value.id.as_str())
            .bind(serde_json::to_string(value)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn insert_import_text_unit(&mut self, value: &ImportTextUnit) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        value.id.validate()?;
        self.context()
            .require_subject(&value.namespace_id, &value.subject_id)?;
        let catalog = read_catalog(
            &mut self.tx,
            &value.namespace_id,
            &value.subject_id,
            &value.catalog_id,
        )
        .await?;
        if value.ordinal >= catalog.original_unit_count
            || value.effective_strategy == ImportTextStrategy::Auto
            || !value.context.is_object()
            || !catalog
                .documents
                .iter()
                .any(|d| d.input_source_id == value.input_source_id)
        {
            return Err(Error::invalid("text unit leaves its frozen catalog"));
        }
        let input = self
            .import_input_source(
                &value.namespace_id,
                &value.subject_id,
                &value.input_source_id,
            )
            .await?;
        let region = self
            .source_region(&value.namespace_id, &value.source_region_id)
            .await?;
        let SourceCoordinate::TextSpan { start, end } = region.coordinate else {
            return Err(Error::invalid("text unit requires an exact UTF-8 TextSpan"));
        };
        if region.material_id != input.material_id
            || value.text.len() as u64 != end - start
            || value.text.is_empty()
        {
            return Err(Error::invalid("text unit differs from its original region"));
        }
        let mut contexts = BTreeSet::new();
        for id in &value.context_region_ids {
            let region = self.source_region(&value.namespace_id, id).await?;
            if !contexts.insert(id)
                || region.material_id != input.material_id
                || !matches!(region.coordinate, SourceCoordinate::TextSpan { .. })
            {
                return Err(Error::invalid(
                    "text unit context leaves its actual input regions",
                ));
            }
        }
        sqlx::query("INSERT INTO import_text_units(id,body) VALUES(?,?)")
            .bind(value.id.as_str())
            .bind(serde_json::to_string(value)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
}
impl ReadView {
    pub async fn import_unit_catalog(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        id: &ImportUnitCatalogId,
    ) -> Result<ImportUnitCatalog> {
        read_catalog(&mut self.tx, ns, subject, id).await
    }
    pub async fn import_text_unit(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        id: &ImportUnitId,
    ) -> Result<ImportTextUnit> {
        let body: Option<String> = sqlx::query_scalar(
            "SELECT body FROM import_text_units WHERE namespace_id=? AND subject_id=? AND id=?",
        )
        .bind(ns.as_str())
        .bind(subject.as_str())
        .bind(id.as_str())
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        Ok(serde_json::from_str(&body.ok_or_else(not_found)?)?)
    }
    pub async fn import_catalog_units(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        catalog: &ImportUnitCatalogId,
    ) -> Result<Vec<ImportTextUnit>> {
        let rows:Vec<String>=sqlx::query_scalar("SELECT body FROM import_text_units WHERE namespace_id=? AND subject_id=? AND catalog_id=? ORDER BY ordinal,id")
            .bind(ns.as_str()).bind(subject.as_str()).bind(catalog.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        rows.into_iter()
            .map(|v| serde_json::from_str(&v).map_err(Error::from))
            .collect()
    }
    pub async fn import_unit_catalog_facts(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<(Vec<ImportUnitCatalog>, Vec<ImportTextUnit>)> {
        let catalogs:Vec<String>=sqlx::query_scalar("SELECT body FROM import_unit_catalogs WHERE namespace_id=? AND subject_id=? ORDER BY commit_seq,id")
            .bind(ns.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let units:Vec<String>=sqlx::query_scalar("SELECT body FROM import_text_units WHERE namespace_id=? AND subject_id=? ORDER BY catalog_id,ordinal,id")
            .bind(ns.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        Ok((
            catalogs
                .into_iter()
                .map(|s| serde_json::from_str(&s).map_err(Error::from))
                .collect::<Result<_>>()?,
            units
                .into_iter()
                .map(|s| serde_json::from_str(&s).map_err(Error::from))
                .collect::<Result<_>>()?,
        ))
    }
}
impl Database {
    pub async fn import_unit_catalog(
        &self,
        ns: &NamespaceId,
        subject: &SubjectId,
        id: &ImportUnitCatalogId,
    ) -> Result<ImportUnitCatalog> {
        let mut view = self.read_view().await?;
        let value = view.import_unit_catalog(ns, subject, id).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn import_text_unit(
        &self,
        ns: &NamespaceId,
        subject: &SubjectId,
        id: &ImportUnitId,
    ) -> Result<ImportTextUnit> {
        let mut view = self.read_view().await?;
        let value = view.import_text_unit(ns, subject, id).await?;
        view.finish().await?;
        Ok(value)
    }
    pub async fn import_catalog_units(
        &self,
        ns: &NamespaceId,
        subject: &SubjectId,
        id: &ImportUnitCatalogId,
    ) -> Result<Vec<ImportTextUnit>> {
        let mut view = self.read_view().await?;
        let value = view.import_catalog_units(ns, subject, id).await?;
        view.finish().await?;
        Ok(value)
    }
}
async fn read_catalog(
    conn: &mut sqlx::SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    id: &ImportUnitCatalogId,
) -> Result<ImportUnitCatalog> {
    let body: Option<String> = sqlx::query_scalar(
        "SELECT body FROM import_unit_catalogs WHERE namespace_id=? AND subject_id=? AND id=?",
    )
    .bind(ns.as_str())
    .bind(subject.as_str())
    .bind(id.as_str())
    .fetch_optional(conn)
    .await
    .map_err(storage_error)?;
    Ok(serde_json::from_str(&body.ok_or_else(not_found)?)?)
}
