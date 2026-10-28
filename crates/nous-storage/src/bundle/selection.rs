use super::selection_graph::Selection;
use crate::{ReadView, model::calls};
use nous_domain::*;
use std::collections::BTreeSet;

impl ReadView {
    pub(super) async fn select_bundle_snapshot(
        &mut self,
        ctx: &CallContext,
        request: &ExportBundleRequest,
        package: &mut SubjectPackage,
    ) -> Result<()> {
        let mut selected = Selection::default();
        let all = matches!(request.selector, BundleSelector::All);
        let native = request.format == BundleExportFormat::Native;
        match &request.selector {
            BundleSelector::All => selected.all(package, native),
            BundleSelector::Materials { material_ids } => {
                for id in material_ids {
                    if !package.materials.iter().any(|v| v.material.id == *id) {
                        return Err(crate::not_found());
                    }
                    selected.add("material", id.as_str());
                }
                select_material_observations(&mut selected, package);
            }
            BundleSelector::Source { sources } => {
                for value in &package.materials {
                    if material_source_name(&value.material, &value.details)
                        .is_some_and(|source| sources.iter().any(|selected| selected == source))
                    {
                        selected.add("material", value.material.id.as_str());
                    }
                }
                select_material_observations(&mut selected, package);
            }
            BundleSelector::Chat { conversation_ref } => {
                for value in &package.observations {
                    // Placement is the current receiving context; original captured
                    // event origin remains the provenance of that receiving event.
                    let placement = self
                        .bundle_observation_placement(
                            &request.namespace_id,
                            &request.subject_id,
                            &value.id,
                        )
                        .await?;
                    let matches = match placement {
                        Some(BundlePlacement::Chat {
                            conversation_ref: actual,
                        }) => actual == *conversation_ref,
                        Some(BundlePlacement::Global) => false,
                        None => {
                            value
                                .origin
                                .as_ref()
                                .and_then(|v| v.conversation_ref.as_ref())
                                == Some(conversation_ref)
                        }
                    };
                    if matches {
                        selected.add("observation", value.id.as_str());
                    }
                }
            }
            BundleSelector::Package {
                package_id,
                installation_id,
            } => {
                let matches: Vec<_> = package
                    .imports
                    .iter()
                    .filter(|layer| {
                        let installation = &layer.installation;
                        installation.namespace_id == request.namespace_id
                            && installation.subject_id == request.subject_id
                            && installation_id
                                .as_ref()
                                .is_none_or(|id| installation.id == *id)
                            && package_id
                                .as_ref()
                                .is_none_or(|id| installation.package.id == *id)
                    })
                    .collect();
                if matches.is_empty() {
                    return Err(crate::not_found());
                }
                for layer in matches {
                    for resource in &layer.resources {
                        selected.add(&resource.kind, &resource.local_id);
                    }
                }
            }
            BundleSelector::PackageToken { token } => {
                let matches: Vec<_> = package
                    .imports
                    .iter()
                    .filter(|layer| {
                        let installation = &layer.installation;
                        installation.namespace_id == request.namespace_id
                            && installation.subject_id == request.subject_id
                            && (installation.id == *token || installation.package.id == *token)
                    })
                    .collect();
                if matches.is_empty() {
                    return Err(crate::not_found());
                }
                for layer in matches {
                    for resource in &layer.resources {
                        selected.add(&resource.kind, &resource.local_id);
                    }
                }
            }
            BundleSelector::Images {
                content_hashes,
                occurrence_ids,
            } => {
                for hash in content_hashes {
                    let values: Vec<_> = package
                        .materials
                        .iter()
                        .filter(|v| {
                            v.material.media_type.starts_with("image/")
                                && v.material.content_hash == *hash
                        })
                        .collect();
                    if values.is_empty() {
                        return Err(crate::not_found());
                    }
                    for value in values {
                        selected.add("material", value.material.id.as_str());
                    }
                }
                for id in occurrence_ids {
                    let mut found = false;
                    for original in &package.materials {
                        if !original.material.media_type.starts_with("image/") {
                            continue;
                        }
                        let preserved: BTreeSet<_> = original
                            .details
                            .metadata
                            .get("image_occurrences")
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter(|record| {
                                record
                                    .pointer("/record/occurrence_id")
                                    .and_then(serde_json::Value::as_str)
                                    == Some(id.as_str())
                            })
                            .filter_map(|record| {
                                record
                                    .get("observation_id")
                                    .and_then(serde_json::Value::as_str)
                            })
                            .collect();
                        for event in &package.observations {
                            if event.material_id == original.material.id
                                && (event.id.as_str() == id
                                    || preserved.contains(event.id.as_str()))
                            {
                                selected.add("observation", event.id.as_str());
                                found = true;
                            }
                        }
                    }
                    if !found {
                        return Err(crate::not_found());
                    }
                }
                if !content_hashes.is_empty() {
                    select_material_observations(&mut selected, package);
                }
            }
            BundleSelector::ImportTask { job_id } => {
                self.seed_import_task(ctx, request, package, &mut selected, job_id, native)
                    .await?
            }
        }
        if !native {
            // Container custody alone does not select portable knowledge.
            // Genuine cognition dependencies can select an independently used original.
            for source in &package.input_sources {
                selected
                    .0
                    .remove(&("material".into(), source.material_id.to_string()));
            }
            selected.0.retain(|(kind, _)| {
                !matches!(
                    kind.as_str(),
                    "import_input_source"
                        | "import_record_container"
                        | "import_unit_catalog"
                        | "import_text_unit"
                        | "import_unit_output"
                        | "import_file_success"
                )
            });
        }
        // Selection and its actual producer/source closure converge inside this ReadView.
        loop {
            let before = selected.0.len();
            selected.related(package);
            if native {
                selected.import_unit_facts(package);
                selected.import_file_facts(package);
                selected.import_sources(package);
            }
            selected.dependencies(package)?;
            if !all {
                selected.semantic_rows(package)?;
            }
            if all {
                for content in &package.vectors.contents {
                    selected.add("model_call", content.model_call_id.as_str());
                }
            }
            let pending: Vec<_> = selected
                .0
                .iter()
                .filter(|(kind, id)| {
                    kind == "model_call" && !package.calls.iter().any(|v| v.id.as_str() == id)
                })
                .map(|(_, id)| ModelCallId(id.clone()))
                .collect();
            for id in pending {
                let call = calls::model_call(&mut self.tx, &id).await?;
                if call.namespace_id != request.namespace_id
                    || call.subject_id != request.subject_id
                {
                    return Err(Error::invalid("bundle producer leaves selected subject"));
                }
                if let Some(sources) = call
                    .request
                    .pointer("/preview/sources")
                    .filter(|v| !v.is_null())
                {
                    let sources: Vec<SourceReadRef> = serde_json::from_value(sources.clone())?;
                    for source in self.imported_model_sources(&call, &sources).await? {
                        selected.read_source(&source)?;
                    }
                }
                package.calls.push(call);
            }
            if selected.0.len() == before {
                break;
            }
        }
        selected.retain(package, all);
        if !all {
            retain_provenance(package, &selected);
        }
        // The caller authorizes only the converged package after this method returns.
        Ok(())
    }
}
fn select_material_observations(selected: &mut Selection, package: &SubjectPackage) {
    for value in &package.observations {
        if selected.has("material", value.material_id.as_str()) {
            selected.add("observation", value.id.as_str());
        }
    }
}
fn retain_provenance(package: &mut SubjectPackage, selected: &Selection) {
    let ns = package.subject.namespace_id.as_str().to_owned();
    let subject = package.subject.id.as_str().to_owned();
    let mut relevant = BTreeSet::new();
    for (kind, id) in &selected.0 {
        relevant.insert((ns.clone(), subject.clone(), kind.clone(), id.clone()));
    }
    loop {
        let before = relevant.len();
        for layer in &package.imports {
            for resource in &layer.resources {
                if relevant.contains(&(
                    layer.installation.namespace_id.to_string(),
                    layer.installation.subject_id.to_string(),
                    resource.kind.clone(),
                    resource.local_id.clone(),
                )) {
                    relevant.insert((
                        layer.installation.source_namespace_id.to_string(),
                        layer.installation.source_subject_id.to_string(),
                        resource.kind.clone(),
                        resource.original_id.clone(),
                    ));
                }
            }
        }
        if before == relevant.len() {
            break;
        }
    }
    for layer in &mut package.imports {
        layer.resources.retain(|r| {
            relevant.contains(&(
                layer.installation.namespace_id.to_string(),
                layer.installation.subject_id.to_string(),
                r.kind.clone(),
                r.local_id.clone(),
            ))
        });
        layer.vector_dispositions.retain(|v| {
            v.mapped_target
                .item()
                .is_some_and(|t| selected.contains_target(t))
        });
    }
    package.imports.retain(|v| !v.resources.is_empty());
}
