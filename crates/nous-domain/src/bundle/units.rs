use super::*;
use std::collections::BTreeSet;

impl SubjectPackage {
    pub(super) fn validate_import_units(&self) -> Result<()> {
        let ns = &self.subject.namespace_id;
        let sub = &self.subject.id;
        let mut catalogs = BTreeSet::new();
        for catalog in &self.unit_catalogs {
            catalog.id.validate()?;
            if catalog.namespace_id != *ns
                || catalog.subject_id != *sub
                || catalog.commit_seq <= 0
                || !catalogs.insert(catalog.id.clone())
                || catalog.original_unit_count == 0
                || catalog.original_unit_count > 1000
                || catalog.documents.is_empty()
            {
                return Err(Error::invalid(
                    "unit catalog has an invalid identity, scope or original unit count",
                ));
            }
            let count = catalog.documents.iter().try_fold(0usize, |sum, document| {
                sum.checked_add(document.original_unit_count)
                    .ok_or_else(|| Error::invalid("document unit count overflow"))
            })?;
            if count != catalog.original_unit_count {
                return Err(Error::invalid(
                    "catalog document counts differ from its original total",
                ));
            }
            let mut documents = BTreeSet::new();
            for document in &catalog.documents {
                if !documents.insert(&document.input_source_id)
                    || document.selected_strategy == ImportTextStrategy::Auto
                    || !self
                        .input_sources
                        .iter()
                        .any(|source| source.id == document.input_source_id)
                {
                    return Err(Error::invalid(
                        "catalog document has no complete admitted input source",
                    ));
                }
                if let Some(time) = &document.effective_options.reference_time {
                    time.validate()?;
                }
            }
        }
        for catalog in &self.unit_catalogs {
            let mut visited = BTreeSet::from([catalog.id.clone()]);
            let mut parent = catalog.parent_catalog_id.as_ref();
            while let Some(id) = parent {
                if !visited.insert(id.clone()) {
                    return Err(Error::invalid("catalog parent references form a cycle"));
                }
                let record = self
                    .unit_catalogs
                    .iter()
                    .find(|value| &value.id == id)
                    .ok_or_else(|| Error::invalid("unit catalog omits its parent header"))?;
                parent = record.parent_catalog_id.as_ref();
            }
        }
        let mut units = BTreeSet::new();
        let mut ordinals = BTreeSet::new();
        for unit in &self.text_units {
            unit.id.validate()?;
            let catalog = self
                .unit_catalogs
                .iter()
                .find(|value| value.id == unit.catalog_id)
                .ok_or_else(|| Error::invalid("text unit has no frozen catalog header"))?;
            let input = self
                .input_sources
                .iter()
                .find(|value| value.id == unit.input_source_id)
                .ok_or_else(|| Error::invalid("text unit has no actual input source"))?;
            if unit.namespace_id != *ns
                || unit.subject_id != *sub
                || unit.commit_seq <= 0
                || !units.insert(unit.id.clone())
                || !ordinals.insert((unit.catalog_id.clone(), unit.ordinal))
                || unit.ordinal >= catalog.original_unit_count
                || unit.text.is_empty()
                || !unit.context.is_object()
                || unit.effective_strategy == ImportTextStrategy::Auto
                || !catalog
                    .documents
                    .iter()
                    .any(|document| document.input_source_id == unit.input_source_id)
            {
                return Err(Error::invalid(
                    "text unit leaves its frozen catalog or subject scope",
                ));
            }
            let region = self
                .source_regions
                .iter()
                .find(|value| value.id == unit.source_region_id)
                .ok_or_else(|| {
                    Error::invalid("text unit has no declared original source region")
                })?;
            let SourceCoordinate::TextSpan { start, end } = region.coordinate else {
                return Err(Error::invalid(
                    "text unit requires an exact original TextSpan",
                ));
            };
            if region.material_id != input.material_id
                || end > input.byte_len
                || end.checked_sub(start) != Some(unit.text.len() as u64)
            {
                return Err(Error::invalid(
                    "text unit length or original source differs from its TextSpan",
                ));
            }
            let mut contexts = BTreeSet::new();
            for id in &unit.context_region_ids {
                let region = self
                    .source_regions
                    .iter()
                    .find(|value| &value.id == id)
                    .ok_or_else(|| Error::invalid("text unit omits an actual context region"))?;
                if !contexts.insert(id)
                    || region.material_id != input.material_id
                    || !matches!(region.coordinate,SourceCoordinate::TextSpan{start,end} if start<end && end<=input.byte_len)
                {
                    return Err(Error::invalid(
                        "text unit context leaves its original input",
                    ));
                }
            }
        }
        for catalog in &self.unit_catalogs {
            for document in &catalog.documents {
                let retained = self
                    .text_units
                    .iter()
                    .filter(|unit| {
                        unit.catalog_id == catalog.id
                            && unit.input_source_id == document.input_source_id
                    })
                    .count();
                if retained > document.original_unit_count {
                    return Err(Error::invalid(
                        "retained document units exceed its original count",
                    ));
                }
            }
        }
        let mut outputs = BTreeSet::new();
        for output in &self.unit_outputs {
            output.id.validate()?;
            if let Some(range) = &output.event_range {
                range.validate()?;
            }
            validate_name(&output.attempt_job_id, "original unit attempt job")?;
            if output.namespace_id != *ns
                || output.subject_id != *sub
                || output.commit_seq <= 0
                || !outputs.insert(output.id.clone())
                || !units.contains(&output.unit_id)
            {
                return Err(Error::invalid(
                    "unit output has no exact scoped retained unit",
                ));
            }
            output.observation_id.validate()?;
            let unit = self
                .text_units
                .iter()
                .find(|unit| unit.id == output.unit_id)
                .ok_or_else(|| Error::invalid("unit output omits original unit"))?;
            let input = self
                .input_sources
                .iter()
                .find(|input| input.id == unit.input_source_id)
                .ok_or_else(|| Error::invalid("unit output omits original input"))?;
            if !self.observations.iter().any(|event| {
                event.id == output.observation_id
                    && event.kind == ObservationKind::Import
                    && event.material_id == input.material_id
            }) {
                return Err(Error::invalid(
                    "unit output omits exact original Import observation",
                ));
            }
            for (id, temporal) in [
                (&output.semantic_call_id, false),
                (&output.temporal_call_id, true),
            ] {
                if let Some(id) = id {
                    let call = self
                        .calls
                        .iter()
                        .find(|value| &value.id == id)
                        .ok_or_else(|| Error::invalid("unit output omits an actual model call"))?;
                    if call.state != ModelCallState::Succeeded
                        && !(temporal
                            && output.temporal_error.is_some()
                            && matches!(
                                call.state,
                                ModelCallState::Failed | ModelCallState::Uncertain
                            ))
                    {
                        return Err(Error::invalid(
                            "unit output differs from its actual producer outcome",
                        ));
                    }
                }
            }
            let mut memories = BTreeSet::new();
            for id in &output.memory_ids {
                if !memories.insert(id)
                    || (!self.memories.iter().any(|value| {
                        &value.id == id
                            && value.observation_id == output.observation_id
                            && value.material_id == input.material_id
                            && value.source
                                == (MediaReference::SourceRegion {
                                    id: unit.source_region_id.clone(),
                                })
                    }) && !self.purges.iter().any(|value| &value.id == id))
                {
                    return Err(Error::invalid(
                        "unit output omits a retained or purged Memory identity",
                    ));
                }
            }
            let mut cognition = BTreeSet::new();
            for reference in &output.cognition {
                if !cognition.insert(reference)
                    || !self
                        .cognition
                        .iter()
                        .any(|value| value.reference() == *reference)
                {
                    return Err(Error::invalid(
                        "unit output omits an exact cognitive result revision",
                    ));
                }
            }
        }
        Ok(())
    }
}
