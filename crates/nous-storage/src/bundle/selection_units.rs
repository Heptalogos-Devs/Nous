use super::selection_graph::Selection;
use nous_domain::*;

impl Selection {
    pub(super) fn import_file_facts(&mut self, p: &SubjectPackage) {
        for success in &p.file_successes {
            // A partial unit selection does not acquire its completed-file siblings.
            if !success.unit_outputs.is_empty()
                && success
                    .unit_outputs
                    .iter()
                    .all(|pair| self.has("import_unit_output", pair.output_id.as_str()))
            {
                self.add("import_file_success", success.id.as_str());
            }
            if self.has("import_file_success", success.id.as_str()) {
                self.add("import_unit_catalog", success.catalog_id.as_str());
                self.add("import_input_source", success.input_source_id.as_str());
                for pair in &success.unit_outputs {
                    self.add("import_text_unit", pair.unit_id.as_str());
                    self.add("import_unit_output", pair.output_id.as_str());
                }
            }
        }
    }

    pub(super) fn import_unit_facts(&mut self, p: &SubjectPackage) {
        for output in &p.unit_outputs {
            if output
                .memory_ids
                .iter()
                .any(|id| self.has("memory", id.as_str()))
                || output.cognition.iter().any(|reference| {
                    self.contains_target(&RetrievalTarget::Cognition(reference.clone()))
                })
            {
                self.add("import_unit_output", output.id.as_str());
            }
            if self.has("import_unit_output", output.id.as_str()) {
                self.add("import_text_unit", output.unit_id.as_str());
                self.add("observation", output.observation_id.as_str());
                for id in &output.memory_ids {
                    self.add("memory", id.as_str());
                }
                for reference in &output.cognition {
                    self.target(&RetrievalTarget::Cognition(reference.clone()));
                }
                for id in [&output.semantic_call_id, &output.temporal_call_id]
                    .into_iter()
                    .flatten()
                {
                    self.add("model_call", id.as_str());
                }
            }
        }
        for unit in &p.text_units {
            if self.has("import_text_unit", unit.id.as_str()) {
                self.add("import_unit_catalog", unit.catalog_id.as_str());
                self.add("import_input_source", unit.input_source_id.as_str());
                self.add("source_region", unit.source_region_id.as_str());
                for id in &unit.context_region_ids {
                    self.add("source_region", id.as_str());
                }
            }
        }
        for catalog in &p.unit_catalogs {
            if self.has("import_unit_catalog", catalog.id.as_str()) {
                if let Some(parent) = &catalog.parent_catalog_id {
                    self.add("import_unit_catalog", parent.as_str());
                }
                for document in &catalog.documents {
                    self.add("import_input_source", document.input_source_id.as_str());
                }
            }
        }
    }
}
