use super::selection_graph::Selection;
use crate::ReadView;
use nous_domain::*;

impl ReadView {
    pub(super) async fn seed_import_task(
        &mut self,
        ctx: &CallContext,
        request: &ExportBundleRequest,
        p: &SubjectPackage,
        selected: &mut Selection,
        id: &str,
        native: bool,
    ) -> Result<()> {
        let job = self.job_record(id).await?;
        job.require_read(ctx)?;
        if job.namespace_id.as_ref() != Some(&request.namespace_id)
            || job.subject_id.as_ref() != Some(&request.subject_id)
        {
            return Err(Error::invalid(
                "bundle import task leaves the requested subject",
            ));
        }
        match job.kind.as_str() {
            "import.text" => {
                let result: ImportResult =
                    serde_json::from_value(job.result.clone().ok_or_else(|| {
                        Error::internal("import task has no admitted result prefix")
                    })?)?;
                if result.job_id != job.id {
                    return Err(Error::internal("import result belongs to another task"));
                }
                if native {
                    for input in &result.inputs {
                        selected.add("import_input_source", input.source_id.as_str());
                    }
                }
                for item in result.items {
                    if let Some(admission) = item.admission {
                        selected.add("material", admission.material_id.as_str());
                        selected.add("observation", admission.observation_id.as_str());
                        selected.add("memory", admission.memory_id.as_str());
                        if let Some(id) = admission.formation_job_id {
                            let formation = self.formation(&id).await?;
                            if formation.job.namespace_id != job.namespace_id
                                || formation.job.subject_id != job.subject_id
                                || formation.material_id != admission.material_id
                                || formation.observation_id != admission.observation_id
                            {
                                return Err(Error::internal(
                                    "import formation result leaves its admitted record",
                                ));
                            }
                            for id in formation.memory_ids {
                                selected.add("memory", id.as_str());
                            }
                        }
                    }
                }
            }
            "import.process" => {
                // Before a parent has submitted its units there are no actual adopted results.
                if let Some(value) = job.result {
                    let result: ImportProcessingResult = serde_json::from_value(value)?;
                    if native {
                        selected.add("import_unit_catalog", result.catalog_id.as_str());
                    }
                    for reference in result.unit_jobs {
                        let unit = p
                            .text_units
                            .iter()
                            .find(|unit| {
                                unit.id == reference.unit_id && unit.catalog_id == result.catalog_id
                            })
                            .ok_or_else(|| {
                                Error::internal("processing result leaves its frozen unit catalog")
                            })?;
                        let child = self.job_record(&reference.job_id).await?;
                        child.require_read(ctx)?;
                        if child.kind != "import.unit"
                            || child.parent_job_id.as_ref() != Some(&job.id)
                            || child.namespace_id != job.namespace_id
                            || child.subject_id != job.subject_id
                        {
                            return Err(Error::internal(
                                "processing result references another unit task",
                            ));
                        }
                        if native {
                            selected.add("import_text_unit", unit.id.as_str());
                        }
                        if let Some(value) = child.result {
                            let result: ImportUnitProcessResult = serde_json::from_value(value)?;
                            select_output(
                                p,
                                selected,
                                &result.output_id,
                                &child.id,
                                Some(&unit.id),
                                native,
                            )?;
                        }
                    }
                }
            }
            "import.unit" => {
                if let Some(value) = job.result {
                    let result: ImportUnitProcessResult = serde_json::from_value(value)?;
                    select_output(p, selected, &result.output_id, &job.id, None, native)?;
                }
            }
            _ => {
                return Err(Error::invalid(
                    "bundle import task must name an import.text, import.process or import.unit task",
                ));
            }
        }
        Ok(())
    }
}
fn select_output(
    p: &SubjectPackage,
    selected: &mut Selection,
    id: &ImportUnitOutputId,
    job: &str,
    unit: Option<&ImportUnitId>,
    native: bool,
) -> Result<()> {
    let output = p
        .unit_outputs
        .iter()
        .find(|output| &output.id == id)
        .ok_or_else(|| Error::internal("unit task output is absent from its source owner"))?;
    if output.attempt_job_id != job || unit.is_some_and(|unit| unit != &output.unit_id) {
        return Err(Error::internal("unit task names another adopted output"));
    }
    if native {
        selected.add("import_unit_output", id.as_str());
    }
    for id in &output.memory_ids {
        selected.add("memory", id.as_str());
    }
    for reference in &output.cognition {
        selected.target(&RetrievalTarget::Cognition(reference.clone()));
    }
    Ok(())
}
