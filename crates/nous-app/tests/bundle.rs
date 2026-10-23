use anyhow::{Context, Result};
use nous_app::bundle_format::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};
use zip::{CompressionMethod, ZipArchive};
use zip::{ZipWriter, write::SimpleFileOptions};

const KNOWLEDGE: &[u8] = include_bytes!("fixtures/bundles/v1-knowledge-1.0.0.amembundle");
const FULL: &[u8] = include_bytes!("fixtures/bundles/v1-full-1.0.0.amembundle");
const IMAGE: &[u8] = include_bytes!("fixtures/bundles/v2-images-1.0.0.amembundle");

#[test]
fn upstream_exports_preserve_complete_inventory() -> Result<()> {
    let knowledge = inspect_bytes(KNOWLEDGE)?;
    assert_eq!(knowledge.manifest.format_version, 1);
    assert_eq!(
        knowledge.knowledge.docs[0].passage,
        "潮汐站在周三升起蓝色旗帜。"
    );
    assert!(knowledge.state.is_none());
    assert_eq!(knowledge.index.knowledge_entities.len(), 2);
    assert_eq!(knowledge.index.knowledge_relations.len(), 1);
    assert_eq!(knowledge.index.vector_members.len(), 2);
    assert_eq!(knowledge.checksums.files.len() + 1, knowledge.members.len());

    let full = inspect_bytes(FULL)?;
    let state = full.state.context("full fixture has no state")?;
    assert_eq!(state.tables.len(), STATE_TABLES.len());
    assert_eq!(state.tables["episodes"][0]["title"], "周三观测");
    assert_eq!(state.tables["fact_claims"][0]["value_text"], "蓝色旗帜");
    assert_eq!(
        state.tables["person_profile_snapshots"][0]["profile_text"],
        "负责潮汐观测"
    );
    for owner in ["graph", "episodes", "facts", "profiles"] {
        assert!(
            full.required_capabilities
                .iter()
                .any(|item| item.capability == owner)
        );
    }

    let image = inspect_bytes(IMAGE)?;
    let records = image
        .images
        .as_ref()
        .context("image fixture has no records")?;
    assert_eq!(records.assets.len(), 1);
    assert_eq!(image.index.vector_members.len(), 3);
    assert_eq!(records.links[0].target_type, "entity");
    assert!(records.observations[0].evidence_json.is_string());
    assert!(image.members.contains_key(&records.assets[0].member));
    assert_eq!(
        records.assets[0].byte_size,
        image.members[&records.assets[0].member].byte_len
    );
    assert!(
        image
            .required_capabilities
            .iter()
            .any(|item| item.capability == "image_embeddings")
    );
    Ok(())
}

fn unpack(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut members = BTreeMap::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index)?;
        let name = member.name()?.into_owned();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes)?;
        members.insert(name, bytes);
    }
    Ok(members)
}

fn pack(members: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in members {
        archive.start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )?;
        archive.write_all(bytes)?;
    }
    Ok(archive.finish()?.into_inner())
}

fn repair_checksums(members: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    let mut manifest: Value = serde_json::from_slice(&members["manifest.json"])?;
    manifest["components"] = json!(
        members
            .keys()
            .filter(|name| !matches!(name.as_str(), "manifest.json" | "checksums.json"))
            .collect::<Vec<_>>()
    );
    members.insert("manifest.json".into(), serde_json::to_vec(&manifest)?);
    let files: BTreeMap<_, _> = members
        .iter()
        .filter(|(name, _)| name.as_str() != "checksums.json")
        .map(|(name, bytes)| (name, format!("{:x}", Sha256::digest(bytes))))
        .collect();
    let checksums = serde_json::to_vec(&json!({"algorithm":"sha256","files":files}))?;
    members.insert("checksums.json".into(), checksums);
    Ok(())
}

#[test]
fn damaged_archives_and_open_asset_references_are_rejected() -> Result<()> {
    let mut members = unpack(KNOWLEDGE)?;
    members.get_mut("knowledge.json").unwrap().push(b' ');
    assert!(
        inspect_bytes(&pack(&members)?)
            .unwrap_err()
            .to_string()
            .contains("checksum mismatch")
    );

    let mut members = unpack(KNOWLEDGE)?;
    members.insert("../escape".into(), b"data".to_vec());
    assert!(
        inspect_bytes(&pack(&members)?)
            .unwrap_err()
            .to_string()
            .contains("unsafe bundle member")
    );

    let mut members = unpack(KNOWLEDGE)?;
    let mut checksums: Value = serde_json::from_slice(&members["checksums.json"])?;
    checksums["files"]
        .as_object_mut()
        .unwrap()
        .remove("manifest.json");
    members.insert("checksums.json".into(), serde_json::to_vec(&checksums)?);
    assert!(
        inspect_bytes(&pack(&members)?)
            .unwrap_err()
            .to_string()
            .contains("coverage")
    );

    let mut members = unpack(IMAGE)?;
    let asset = members
        .keys()
        .find(|name| name.starts_with("images/assets/"))
        .cloned()
        .unwrap();
    members.remove(&asset);
    repair_checksums(&mut members)?;
    assert!(
        inspect_bytes(&pack(&members)?)
            .unwrap_err()
            .to_string()
            .contains("image asset member is missing")
    );

    let mut members = unpack(FULL)?;
    members.remove("state.json");
    repair_checksums(&mut members)?;
    assert!(
        inspect_bytes(&pack(&members)?)
            .unwrap_err()
            .to_string()
            .contains("full bundle requires state")
    );

    Ok(())
}

#[test]
fn archive_directory_and_json_limits_are_enforced() -> Result<()> {
    for limits in [
        Limits {
            archive_bytes: 1,
            ..Limits::default()
        },
        Limits {
            members: 1,
            ..Limits::default()
        },
        Limits {
            member_bytes: 1,
            ..Limits::default()
        },
        Limits {
            expanded_bytes: 1,
            ..Limits::default()
        },
        Limits {
            central_directory_bytes: 1,
            ..Limits::default()
        },
        Limits {
            json_bytes: 1,
            ..Limits::default()
        },
        Limits {
            json_nodes: 1,
            ..Limits::default()
        },
        Limits {
            json_depth: 1,
            ..Limits::default()
        },
    ] {
        assert!(inspect(Cursor::new(KNOWLEDGE), &limits).is_err());
    }
    let mut footer_forgery = KNOWLEDGE.to_vec();
    let offset = footer_forgery.len() - 22 + 12;
    footer_forgery[offset..offset + 4].copy_from_slice(&1u32.to_le_bytes());
    assert!(
        inspect_bytes(&footer_forgery)
            .unwrap_err()
            .to_string()
            .contains("central directory record")
    );

    Ok(())
}
