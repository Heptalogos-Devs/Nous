use super::*;
use std::collections::BTreeSet;

impl SubjectPackage {
    pub(super) fn validate_import_files(&self) -> Result<()> {
        let (ns, sub) = (&self.subject.namespace_id, &self.subject.id);
        let mut identities = BTreeSet::new();
        for fact in &self.file_successes {
            fact.id.validate()?;
            validate_name(&fact.processing_job_id, "historical file processing job")?;
            if let Some(origin) = &fact.origin {
                origin.validate()?;
            }
            if let Some(identity) = &fact.file_identity {
                validate_file_identity(identity)?;
            }
            if fact.namespace_id != *ns
                || fact.subject_id != *sub
                || fact.commit_seq <= 0
                || !identities.insert(fact.id.clone())
            {
                return Err(Error::invalid(
                    "file success has an invalid identity or subject scope",
                ));
            }
            let catalog = self
                .unit_catalogs
                .iter()
                .find(|catalog| catalog.id == fact.catalog_id)
                .ok_or_else(|| Error::invalid("file success omits its frozen catalog"))?;
            let document = catalog
                .documents
                .iter()
                .find(|document| document.input_source_id == fact.input_source_id)
                .ok_or_else(|| Error::invalid("file success leaves its catalog document"))?;
            if fact.unit_outputs.is_empty()
                || fact.unit_outputs.len() != document.original_unit_count
            {
                return Err(Error::invalid(
                    "file success requires every original document unit",
                ));
            }
            let mut units = BTreeSet::new();
            let mut outputs = BTreeSet::new();
            for pair in &fact.unit_outputs {
                let unit = self
                    .text_units
                    .iter()
                    .find(|unit| unit.id == pair.unit_id)
                    .ok_or_else(|| Error::invalid("file success omits an exact original unit"))?;
                let output = self
                    .unit_outputs
                    .iter()
                    .find(|output| output.id == pair.output_id)
                    .ok_or_else(|| Error::invalid("file success omits its actual unit output"))?;
                if !units.insert(&pair.unit_id)
                    || !outputs.insert(&pair.output_id)
                    || unit.catalog_id != fact.catalog_id
                    || unit.input_source_id != fact.input_source_id
                    || output.unit_id != unit.id
                {
                    return Err(Error::invalid(
                        "file success repeats or mismatches an original unit output",
                    ));
                }
                self.validate_file_output_origin(fact, unit, output)?;
            }
        }
        let mut heads = BTreeSet::new();
        for head in &self.import_manifest_heads {
            if head.namespace_id != *ns
                || head.subject_id != *sub
                || head.commit_seq <= 0
                || head.policy == ImportFileDedupePolicy::None
                || !heads.insert((
                    serde_json::to_string(&head.origin)?,
                    serde_json::to_string(&head.policy)?,
                    serde_json::to_string(&head.key)?,
                ))
            {
                return Err(Error::invalid(
                    "manifest qualification has an invalid or repeated scoped key",
                ));
            }
            if let Some(origin) = &head.origin {
                origin.validate()?;
            }
            let fact = self
                .file_successes
                .iter()
                .find(|fact| fact.id == head.success_id)
                .ok_or_else(|| {
                    Error::invalid("manifest qualification omits its actual file success")
                })?;
            let input = self
                .input_sources
                .iter()
                .find(|input| input.id == fact.input_source_id)
                .ok_or_else(|| Error::invalid("manifest qualification omits original content"))?;
            if head.origin != fact.origin
                || head.policy != fact.dedupe_policy
                || head
                    .policy
                    .key(&input.content_hash, fact.file_identity.as_deref())
                    .as_ref()
                    != Some(&head.key)
            {
                return Err(Error::invalid(
                    "manifest qualification differs from actual successful file",
                ));
            }
        }
        Ok(())
    }
    fn validate_file_output_origin(
        &self,
        fact: &ImportFileSuccess,
        unit: &ImportTextUnit,
        output: &ImportUnitOutput,
    ) -> Result<()> {
        let input = self
            .input_sources
            .iter()
            .find(|input| input.id == fact.input_source_id)
            .ok_or_else(|| Error::invalid("file success omits actual original bytes"))?;
        if output.memory_ids.is_empty() {
            return Err(Error::invalid(
                "file output requires its actual Memory admission",
            ));
        }
        let observation = self
            .observations
            .iter()
            .find(|observation| observation.id == output.observation_id)
            .ok_or_else(|| {
                Error::invalid("file output omits exact original admission observation")
            })?;
        if observation.kind != ObservationKind::Import
            || observation.material_id != input.material_id
            || self.original_file_origin(observation)? != fact.origin
        {
            return Err(Error::invalid(
                "file success origin differs from its actual original admission",
            ));
        }
        for id in &output.memory_ids {
            let memory = self.memories.iter().find(|memory| {
                memory.id == *id
                    && memory.observation_id == output.observation_id
                    && memory.material_id == input.material_id
                    && memory.source
                        == (MediaReference::SourceRegion {
                            id: unit.source_region_id.clone(),
                        })
            });
            if memory.is_none() && !self.purges.iter().any(|purge| purge.id == *id) {
                return Err(Error::invalid(
                    "file output has no exact original or purged Memory admission",
                ));
            }
        }
        Ok(())
    }
    fn original_file_origin(&self, observation: &Observation) -> Result<Option<ObservationOrigin>> {
        let mut current = observation;
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert((
                current.namespace_id.clone(),
                current.subject_id.clone(),
                current.id.clone(),
            )) || seen.len() > 64
            {
                return Err(Error::invalid(
                    "file origin observation provenance forms a cycle",
                ));
            }
            let previous = self
                .imports
                .iter()
                .filter(|layer| {
                    layer.installation.namespace_id == current.namespace_id
                        && layer.installation.subject_id == current.subject_id
                })
                .find_map(|layer| {
                    layer
                        .resources
                        .iter()
                        .find(|resource| {
                            resource.kind == "observation"
                                && resource.local_id == current.id.as_str()
                        })
                        .map(|resource| (layer, resource))
                });
            let Some((layer, resource)) = previous else {
                return Ok(current.origin.clone());
            };
            let original = resource.original_observation.as_ref().ok_or_else(|| {
                Error::invalid("imported file admission omits original observation provenance")
            })?;
            if original.id.as_str() != resource.original_id
                || original.namespace_id != layer.installation.source_namespace_id
                || original.subject_id != layer.installation.source_subject_id
                || original.kind != ObservationKind::Import
            {
                return Err(Error::invalid(
                    "file admission provenance differs from its declared source scope",
                ));
            }
            current = original;
        }
    }
}
fn validate_file_identity(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(Error::invalid(
            "logical file identity requires 1..4096 bytes without controls",
        ));
    }
    Ok(())
}
