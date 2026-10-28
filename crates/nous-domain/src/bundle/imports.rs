use super::*;
use std::collections::BTreeSet;

impl SubjectPackage {
    pub(super) fn import_input_blob(&self, source: &ImportInputSource) -> Result<BlobReference> {
        let original = self
            .materials
            .iter()
            .find(|v| v.material.id == source.material_id)
            .ok_or_else(|| {
                Error::invalid("import input source has no declared original Material")
            })?;
        if original.material.content_hash != source.content_hash
            || u64::try_from(original.material.byte_len).ok() != Some(source.byte_len)
        {
            return Err(Error::invalid(
                "import input source differs from its original bytes",
            ));
        }
        Ok(BlobReference {
            content_hash: source.content_hash.clone(),
            media_type: original.material.media_type.clone(),
            byte_len: source.byte_len,
        })
    }
    pub(super) fn validate_import_sources(&self) -> Result<()> {
        let (namespace, subject) = (&self.subject.namespace_id, &self.subject.id);
        let mut sources = BTreeSet::new();
        for source in &self.input_sources {
            source.id.validate()?;
            source.material_id.validate()?;
            validate_name(&source.original_job_id, "original import job")?;
            if source.namespace_id != *namespace
                || source.subject_id != *subject
                || source.commit_seq <= 0
                || !sources.insert(source.id.clone())
            {
                return Err(Error::invalid(
                    "import input source identity or subject scope is invalid",
                ));
            }
            if source.name.as_ref().is_some_and(|name| {
                name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control)
            }) {
                return Err(Error::invalid(
                    "import input name must contain 1 to 1024 bytes without controls",
                ));
            }
            validate_name(&source.recorded_at, "import input recorded_at")?;
            self.import_input_blob(source)?.validate()?;
        }
        let mut containers = BTreeSet::new();
        for container in &self.record_containers {
            container.id.validate()?;
            container.input_source_id.validate()?;
            container.record_material_id.validate()?;
            container.observation_id.validate()?;
            container.memory_id.validate()?;
            validate_name(&container.original_job_id, "original record import job")?;
            validate_name(&container.recorded_at, "record container recorded_at")?;
            if container.namespace_id != *namespace
                || container.subject_id != *subject
                || container.commit_seq <= 0
                || !containers.insert(container.id.clone())
                || !sources.contains(&container.input_source_id)
            {
                return Err(Error::invalid(
                    "import record container identity, input source or subject scope is invalid",
                ));
            }
            if !self
                .materials
                .iter()
                .any(|v| v.material.id == container.record_material_id)
                || !self.observations.iter().any(|v| {
                    v.id == container.observation_id
                        && v.material_id == container.record_material_id
                        && v.kind == ObservationKind::Import
                })
            {
                return Err(Error::invalid(
                    "import record container has no exact admitted source event",
                ));
            }
            let retained = self.memories.iter().any(|v| {
                v.id == container.memory_id
                    && v.observation_id == container.observation_id
                    && v.material_id == container.record_material_id
            });
            let purged = self.purges.iter().any(|v| v.id == container.memory_id);
            if !retained && !purged {
                return Err(Error::invalid(
                    "import record container has no retained or purged Memory identity",
                ));
            }
        }
        Ok(())
    }
}
