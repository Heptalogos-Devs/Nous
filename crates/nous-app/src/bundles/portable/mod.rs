mod images;
mod semantic;
mod source_rows;
mod supplement;
mod vectors;

use crate::bundle_format::{self, canonical_json};
use anyhow::{Context, ensure};
pub use nous_domain::BundleAssetOptions as PortableOptions;
use nous_domain::{Error, Result, SubjectPackage};
use nous_foundation::execution;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;
use zip::{ZipWriter, write::SimpleFileOptions};

pub struct PortableSummary {
    pub manifest: Value,
    pub counts: BTreeMap<String, u64>,
    pub uncompressed_size: u64,
}
pub struct PortableArtifact {
    pub path: PathBuf,
    pub summary: PortableSummary,
}
enum Member {
    Bytes(Vec<u8>),
    Blob {
        path: PathBuf,
        size: u64,
        hash: String,
    },
}
impl Member {
    fn size(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::Blob { size, .. } => *size,
        }
    }
    fn hash(&self) -> String {
        match self {
            Self::Bytes(bytes) => hash(bytes),
            Self::Blob { hash, .. } => hash.clone(),
        }
    }
}
struct Projection {
    members: BTreeMap<String, Member>,
    summary: PortableSummary,
}
pub fn preview(
    root: &Path,
    package: &SubjectPackage,
    options: &PortableOptions,
) -> Result<PortableSummary> {
    project(root, package, options)
        .map(|p| p.summary)
        .map_err(format_error)
}
pub fn write(
    root: &Path,
    package: &SubjectPackage,
    options: &PortableOptions,
    cancel: &CancellationToken,
) -> Result<PortableArtifact> {
    execution::check_cancelled(cancel)?;
    let projection = project(root, package, options).map_err(format_error)?;
    execution::check_cancelled(cancel)?;
    let path = root.join("subject.amembundle");
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(&path)?;
    let result = (|| -> Result<()> {
        let mut zip = ZipWriter::new(file);
        for (name, member) in projection.members {
            execution::check_cancelled(cancel)?;
            zip.start_file(
                name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .map_err(|e| Error::invalid("cannot write portable archive").with_cause(&e))?;
            match member {
                Member::Bytes(bytes) => zip.write_all(&bytes)?,
                Member::Blob {
                    path,
                    size,
                    hash: expected,
                } => {
                    let mut file = File::open(path)?;
                    let mut digest = Sha256::new();
                    let mut total = 0u64;
                    let mut buffer = [0; 65536];
                    loop {
                        execution::check_cancelled(cancel)?;
                        let count = file.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        total += count as u64;
                        if total > size {
                            return Err(Error::invalid("portable asset changed during export"));
                        }
                        digest.update(&buffer[..count]);
                        zip.write_all(&buffer[..count])?;
                    }
                    if total != size
                        || format!("{digest:x}", digest = digest.finalize()) != expected
                    {
                        return Err(Error::invalid("portable asset changed during export"));
                    }
                }
            }
        }
        zip.finish()
            .map_err(|e| Error::invalid("cannot finish portable archive").with_cause(&e))?
            .sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(PortableArtifact {
        path,
        summary: projection.summary,
    })
}
fn project(
    root: &Path,
    package: &SubjectPackage,
    options: &PortableOptions,
) -> anyhow::Result<Projection> {
    package.package.validate()?;
    ensure!(
        package
            .package
            .version
            .as_ref()
            .is_some_and(|v| !v.trim().is_empty()),
        "A-Memorix requires a publication version"
    );
    let semantic = semantic::project(root, package)?;
    let mut members = BTreeMap::new();
    members.insert(
        "knowledge.json".into(),
        Member::Bytes(canonical_json(&semantic.knowledge)?),
    );
    if let Some(state) = &semantic.state {
        members.insert("state.json".into(), Member::Bytes(canonical_json(state)?));
    }
    let semantic_retention = supplement::project(root, package, &mut members)?;
    let mut images = images::project(root, package, options, &semantic, &mut members)?;
    let vector_result = vectors::project(
        root,
        package,
        options,
        &semantic,
        images.as_ref(),
        &mut members,
    )?;
    if let Some(images) = &mut images {
        images.fingerprint = vector_result.image_fingerprint.clone();
    }
    let mut counts = semantic.counts.clone();
    let mut payload = json!({"knowledge":semantic.knowledge,"state":semantic.state});
    if let Some(images) = &images {
        payload["images"] = bundle_format::canonical_images(
            images,
            &mut bundle_format::Limits::default().json_nodes,
            64,
        )?;
        for (key, value) in [
            ("image_assets", images.assets.len()),
            ("image_occurrences", images.occurrences.len()),
            ("image_observations", images.observations.len()),
            ("image_links", images.links.len()),
        ] {
            counts.insert(key.into(), value as u64);
        }
        members.insert(
            "images.json".into(),
            Member::Bytes(canonical_json(&serde_json::to_value(images)?)?),
        );
    }
    ensure!(
        !semantic.paragraphs.is_empty()
            || images.is_some()
            || semantic_retention["record_count"]
                .as_u64()
                .is_some_and(|v| v > 0),
        "selection has no portable text or image knowledge"
    );
    let manifest = json!({
        "format":bundle_format::FORMAT,"format_version":if images.is_some(){2}else{1},
        "created_at":package.created_at,"package":package.package,"content_level":package.content_level,
        "content_digest":hash(&canonical_json(&payload)?),"counts":counts,
        "components":members.keys().collect::<Vec<_>>(),
        "embedding_fingerprint":vector_result.text_fingerprint,"image_embedding_fingerprint":vector_result.image_fingerprint,
        "compatibility":{"knowledge_semantics":"lpmm_openie","minimum_a_memorix_bundle_version":1},
        "retention":{"native_cognition_without_standard_mapping":semantic.unmapped,
            "nous_semantics":semantic_retention,
            "native_vectors":vector_result.retention,"original_materials":"Only selected image assets are included; Native exports retain complete original sources"}
    });
    members.insert(
        "manifest.json".into(),
        Member::Bytes(canonical_json(&manifest)?),
    );
    let checksums: BTreeMap<_, _> = members
        .iter()
        .map(|(name, member)| (name, member.hash()))
        .collect();
    members.insert(
        "checksums.json".into(),
        Member::Bytes(canonical_json(
            &json!({"algorithm":"sha256","files":checksums}),
        )?),
    );
    let limits = bundle_format::Limits::default();
    let mut remaining_nodes = limits.json_nodes;
    for (name, member) in &members {
        if name.ends_with(".json")
            && let Member::Bytes(bytes) = member
        {
            ensure!(
                bytes.len() as u64 <= limits.json_bytes,
                "portable JSON member exceeds format limit"
            );
            nous_foundation::json::bounded_value(bytes, &mut remaining_nodes, limits.json_depth)?;
        }
    }
    let uncompressed_size = members.values().map(Member::size).sum::<u64>();
    ensure!(
        members.len() <= limits.members
            && uncompressed_size <= limits.expanded_bytes
            && members.values().all(|m| m.size() <= limits.member_bytes),
        "portable archive exceeds configured format limits"
    );
    Ok(Projection {
        members,
        summary: PortableSummary {
            manifest,
            counts,
            uncompressed_size,
        },
    })
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn format_error(error: anyhow::Error) -> Error {
    if let Some(domain) = error.downcast_ref::<Error>()
        && domain.code == nous_domain::ErrorCode::Conflict
    {
        return domain.clone();
    }
    Error::invalid("cannot project A-Memorix bundle").with_cause(error.as_ref())
}
fn blob(root: &Path, content_hash: &str, expected_size: u64) -> anyhow::Result<Member> {
    ensure!(
        bundle_format::valid_hash(content_hash),
        "invalid portable source hash"
    );
    let path = root.join(content_hash);
    let mut file = File::open(&path).context("portable source bytes are missing")?;
    ensure!(
        file.metadata()?.len() == expected_size,
        "portable source size differs from metadata"
    );
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    ensure!(
        format!("{:x}", digest.finalize()) == content_hash,
        "portable source content hash mismatch"
    );
    Ok(Member::Blob {
        path,
        size: expected_size,
        hash: content_hash.into(),
    })
}
fn read_declared(
    root: &Path,
    material: &nous_domain::media::MediaOriginal,
) -> anyhow::Result<Vec<u8>> {
    let size = u64::try_from(material.material.byte_len)?;
    ensure!(
        size <= bundle_format::Limits::default().member_bytes,
        "semantic source exceeds portable member limit"
    );
    let Member::Blob { path, .. } = blob(root, &material.material.content_hash, size)? else {
        unreachable!()
    };
    Ok(std::fs::read(path)?)
}
