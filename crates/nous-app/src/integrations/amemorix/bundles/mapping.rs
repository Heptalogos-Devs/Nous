use super::input::{ExportInput, ImportInput};
use nous_domain::*;
use serde_json::Value;

pub(super) fn export_request(
    ns: &NamespaceId,
    subject: &SubjectId,
    input: &ExportInput,
) -> Result<ExportBundleRequest> {
    let package = &input.package;
    Ok(ExportBundleRequest {
        namespace_id: ns.clone(),
        subject_id: subject.clone(),
        package: PackageDescriptor {
            id: text(&package.id, &package.package_id, "amx.preview"),
            name: text(&package.name, "", "A_Memorix 知识包"),
            version: Some(text(&package.version, "", "1.0.0")),
            description: package.description.trim().into(),
            author: package.author.trim().into(),
        },
        content_level: input.content_level,
        format: BundleExportFormat::Amemorix,
        selector: selector(&input.selector)?,
        assets: Some(BundleAssetOptions {
            include_vectors: input.include_vectors,
            include_images: input.include_images,
            include_image_related: input.include_image_related,
        }),
    })
}

pub(super) fn automatic_id(input: &ExportInput) -> bool {
    input.package.id.trim().is_empty() && input.package.package_id.trim().is_empty()
}

pub(super) fn placement(input: &ImportInput) -> Result<BundlePlacement> {
    let chat = input.chat_id.trim();
    match input.scope_type.trim().to_ascii_lowercase().as_str() {
        "global" if chat.is_empty() => Ok(BundlePlacement::Global),
        "chat" if !chat.is_empty() => Ok(BundlePlacement::Chat {
            conversation_ref: ConversationRef::new(chat.to_owned())?,
        }),
        _ => Err(Error::invalid(
            "scope_type and chat_id do not describe a valid installation scope",
        )),
    }
}

fn text(first: &str, second: &str, default: &str) -> String {
    [first, second]
        .into_iter()
        .map(str::trim)
        .find(|v| !v.is_empty())
        .unwrap_or(default)
        .into()
}

fn token(value: &Value, names: &[&str]) -> String {
    names
        .iter()
        .filter_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::trim)
        .find(|v| !v.is_empty())
        .unwrap_or("")
        .into()
}
fn strings(value: &Value, names: &[&str]) -> Result<Vec<String>> {
    for name in names {
        if let Some(items) = value.get(name) {
            let items = items
                .as_array()
                .ok_or_else(|| Error::invalid("selector values must be an array"))?;
            return items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(|v| v.trim().to_owned())
                        .ok_or_else(|| Error::invalid("selector value must be text"))
                })
                .collect();
        }
    }
    Ok(vec![])
}

fn selector(value: &Value) -> Result<BundleSelector> {
    if !value.is_object() {
        return Err(Error::invalid("selector must be an object"));
    }
    let kind = token(value, &["type"]);
    Ok(match kind.as_str() {
        "" | "all" => BundleSelector::All,
        "chat" => BundleSelector::Chat {
            conversation_ref: ConversationRef::new(token(value, &["chat_id", "value"]))?,
        },
        "source" => {
            let mut sources = strings(value, &["values", "sources"])?;
            if sources.is_empty() {
                sources.push(token(value, &["value", "source"]));
            }
            BundleSelector::Source { sources }
        }
        "package" => {
            let installation = token(value, &["installation_id"]);
            let package = token(value, &["package_id"]);
            if !installation.is_empty() || !package.is_empty() {
                BundleSelector::Package {
                    installation_id: (!installation.is_empty()).then_some(installation),
                    package_id: (!package.is_empty()).then_some(package),
                }
            } else {
                BundleSelector::PackageToken {
                    token: token(value, &["value"]),
                }
            }
        }
        "import_task" => BundleSelector::ImportTask {
            job_id: token(value, &["task_id", "job_id", "value"]),
        },
        "materials" => BundleSelector::Materials {
            material_ids: strings(value, &["material_ids", "values"])?
                .into_iter()
                .map(|v| v.parse())
                .collect::<Result<_>>()?,
        },
        "image" | "images" => BundleSelector::Images {
            content_hashes: strings(value, &["content_hashes", "values"])?,
            occurrence_ids: strings(value, &["occurrence_ids"])?,
        },
        _ => {
            return Err(
                Error::invalid("unsupported bundle selector").with_context("selector", kind)
            );
        }
    })
}
