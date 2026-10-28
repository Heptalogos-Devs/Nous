use super::*;
use nous_domain::media::{MediaReference, SourceCoordinate};

impl App {
    pub async fn import_catalog(
        &self,
        ctx: &CallContext,
        request: ReadImportCatalogRequest,
        cancel: &CancellationToken,
    ) -> Result<ImportCatalogView> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        let mut view = self.database.read_view().await?;
        let catalog = view
            .import_unit_catalog(
                &request.namespace_id,
                &request.subject_id,
                &request.catalog_id,
            )
            .await?;
        let units = view
            .import_catalog_units(
                &request.namespace_id,
                &request.subject_id,
                &request.catalog_id,
            )
            .await?;
        for unit in &units {
            self.verify_catalog_unit(ctx, &mut view, unit, cancel)
                .await?;
        }
        view.finish().await?;
        Ok(ImportCatalogView {
            catalog,
            retained_unit_count: units.len(),
            units,
        })
    }
    /// Resolve an explicit retry selection without re-splitting, creating Jobs, or changing IDs.
    pub async fn retry_import_inputs(
        &self,
        ctx: &CallContext,
        request: RetryImportInputsRequest,
        cancel: &CancellationToken,
    ) -> Result<ImportUnitInputs> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        if request.unit_ids.is_empty()
            || request.unit_ids.len() > import_format::MAX_IMPORT_RECORDS
            || request.unit_ids.iter().collect::<BTreeSet<_>>().len() != request.unit_ids.len()
        {
            return Err(Error::invalid(
                "retry input selection requires 1 to 1000 distinct frozen UnitIds",
            ));
        }
        let mut view = self.database.read_view().await?;
        let catalog = view
            .import_unit_catalog(
                &request.namespace_id,
                &request.subject_id,
                &request.catalog_id,
            )
            .await?;
        let mut units = Vec::new();
        let mut source_refs = BTreeSet::new();
        for id in &request.unit_ids {
            let unit = view
                .import_text_unit(&request.namespace_id, &request.subject_id, id)
                .await?;
            if unit.catalog_id != catalog.id {
                return Err(Error::invalid(
                    "retry UnitId belongs to another frozen catalog",
                ));
            }
            self.verify_catalog_unit(ctx, &mut view, &unit, cancel)
                .await?;
            source_refs.extend(unit.source_requirements());
            units.push(unit);
        }
        units.sort_by_key(|unit| unit.ordinal);
        view.finish().await?;
        Ok(ImportUnitInputs {
            catalog,
            units,
            source_refs: source_refs.into_iter().collect(),
        })
    }
    async fn verify_catalog_unit(
        &self,
        ctx: &CallContext,
        view: &mut nous_storage::ReadView,
        unit: &ImportTextUnit,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let heading = unit
            .context
            .get("heading_range")
            .and_then(serde_json::Value::as_object);
        let mut heading_verified = heading.is_none();
        for (index, id) in std::iter::once(&unit.source_region_id)
            .chain(unit.context_region_ids.iter())
            .enumerate()
        {
            let source = self
                .sources
                .resolve_in(
                    ctx,
                    view,
                    &unit.namespace_id,
                    &unit.subject_id,
                    &MediaReference::SourceRegion { id: id.clone() },
                    None,
                )
                .await?;
            let text = self.sources.text(&source, cancel).await?;
            if index == 0 && text != unit.text {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "retained unit text differs from its original source region",
                ));
            }
            if index > 0
                && let Some(heading) = heading
            {
                let region = view.source_region(&unit.namespace_id, id).await?;
                if let SourceCoordinate::TextSpan { start, end } = region.coordinate
                    && heading
                        .get("start_byte")
                        .and_then(serde_json::Value::as_u64)
                        == Some(start)
                    && heading.get("end_byte").and_then(serde_json::Value::as_u64) == Some(end)
                {
                    heading_verified = true;
                    if unit
                        .context
                        .get("scene_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|title| title != text.trim())
                    {
                        return Err(Error::new(
                            ErrorCode::Conflict,
                            "retained scene title differs from its actual context source",
                        ));
                    }
                }
            }
        }
        if !heading_verified {
            return Err(Error::new(
                ErrorCode::Conflict,
                "retained scene title has no exact authorized heading region",
            ));
        }
        Ok(())
    }
}
