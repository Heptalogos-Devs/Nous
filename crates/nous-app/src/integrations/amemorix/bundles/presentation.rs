use super::input::ImportInput;
use crate::{OperationReceipt, bundles::BundleInspectionSummary};
use nous_domain::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn inspection(summary: BundleInspectionSummary) -> Value {
    let has_image_vectors = (summary.format == "a-memorix-memory-bundle").then(|| {
        summary
            .vector_members
            .iter()
            .any(|name| name == "vectors/images.npz")
    });
    let file_name = summary
        .material
        .source
        .as_deref()
        .and_then(|source| std::path::Path::new(source).file_name())
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    let mut unknown = json!({
        "images.vector_compatible":"No legacy runtime compatibility decision was requested",
        "images.runtime_status":"Archive inspection does not probe the receiving image runtime",
        "images.missing_resources":"Receiving image readiness is not part of archive inspection"
    });
    if has_image_vectors.is_none() {
        unknown["images.has_vectors"] =
            json!("Standard image vector members are reported only for A-Memorix archives");
    }
    json!({"success":true,"manifest":summary.manifest,"package":summary.package,
        "content_level":summary.content_level,"counts":summary.counts,
        "file_name":file_name,"size":summary.material.byte_len,
        "material_id":summary.material.id,
        "images":{"vector_compatible":null,"has_vectors":has_image_vectors,
            "runtime_status":null,"missing_resources":null,"assets":summary.image_assets},
        "unknown":unknown,
        "native":summary})
}

pub(super) fn installation(value: BundleInstallation) -> Result<Value> {
    let chat = value
        .placement
        .conversation()
        .map(|chat| chat.as_str())
        .unwrap_or("");
    let installed_at = timestamp(&value.installed_at)?;
    let updated_at = timestamp(
        value
            .uninstalled_at
            .as_deref()
            .unwrap_or(&value.installed_at),
    )?;
    let status = if value.uninstalled_at.is_some() {
        "uninstalled"
    } else {
        "installed"
    };
    Ok(
        json!({"installation_id":value.id,"package_id":value.package.id,
        "version":value.package.version,"name":value.package.name,
        "content_level":value.content_level,"content_digest":value.content_digest,
        "scope_type":value.placement.kind(),"scope_key":if chat.is_empty(){"global"}else{chat},
        "chat_id":chat,"status":status,"installed_at":installed_at,"updated_at":updated_at,
        "paragraph_count":value.resource_counts.get("memory").copied().unwrap_or(0),
        "images":{"assets":null,"occurrences":null,"ready":null,
            "content_status":status,"retrieval_status":null},
        "unknown":{"images":"Installation resources do not report image-specific readiness"},
        "native":value}),
    )
}

pub(super) fn installed(receipt: OperationReceipt, input: &ImportInput) -> Result<Value> {
    let value = &receipt.result;
    let installation: BundleInstallation = serde_json::from_value(value["installation"].clone())?;
    Ok(
        json!({"success":true,"already_installed":installation.commit_seq != receipt.commit_seq,
        "installation_id":installation.id,"package":installation.package,
        "content_level":installation.content_level,"mode":input.mode,
        "scope_type":installation.placement.kind(),
        "chat_id":installation.placement.conversation().map(|chat|chat.as_str()).unwrap_or(""),
        "inserted":null,"mapped_paragraphs":installation.resource_counts.get("memory").copied().unwrap_or(0),
        "vectors":{"bundle_compatible":null,"imported":null,"generated":null,
            "native_dispositions":value["vectors"],"native_contents":value["vector_contents"],
            "unaccepted_external_assets":value["unaccepted_external_vector_assets"]},
        "graph":null,"images":null,
        "unknown":{"inserted":"Native installation reports retained resources, not per-table new rows",
            "vectors":"Native space acceptance and external legacy fingerprint compatibility have distinct semantics",
            "graph":"No legacy graph rebuild is performed",
            "images":"Image runtime readiness is not included in the installation receipt"},
        "operation_id":receipt.operation_id,"native":receipt}),
    )
}

pub(super) fn uninstalled(receipt: OperationReceipt) -> Result<Value> {
    let value = &receipt.result;
    let installation: BundleInstallation = serde_json::from_value(value["installation"].clone())?;
    let targets: Vec<RetrievalTarget> = serde_json::from_value(value["withdrawn"].clone())?;
    let mut identities = BTreeSet::new();
    let mut removed = BTreeMap::<String, usize>::new();
    let mut removed_memories = false;
    for target in targets {
        if !identities.insert(target.identity_key()?) {
            continue;
        }
        let name = match target.cognition().map(|reference| reference.kind) {
            Some(CognitiveKind::Memory) => "paragraphs",
            Some(CognitiveKind::Entity) => "entities",
            Some(CognitiveKind::Association) => "relations",
            Some(CognitiveKind::Episode) => "episodes",
            Some(_) => "other_cognition",
            None => "media_resources",
        };
        removed_memories |= target.cognition().is_some();
        *removed.entry(name.into()).or_default() += 1;
    }
    Ok(json!({"success":true,"installation_id":installation.id,
        "removed_memories":removed_memories,"removed":removed,"retained_shared":null,
        "removed_vectors":null,"removed_images":null,"graph":null,
        "message":"知识包已卸载，资源按实际归属撤回",
        "unknown":{"retained_shared":"Withdrawal receipt does not enumerate retained shared resources",
            "removed_vectors":"Withdrawal receipt does not count retired vector associations",
            "removed_images":"Withdrawal receipt does not classify withdrawn media as image assets",
            "graph":"No legacy graph rebuild is performed"},
        "operation_id":receipt.operation_id,"native":receipt}))
}

fn timestamp(value: &str) -> Result<f64> {
    Ok(OperationTime::parse(value)?.utc().timestamp_millis() as f64 / 1000.0)
}
