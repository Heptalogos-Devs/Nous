use crate::bundle_format::{self, Checksums, Limits, Member};
use nous_domain::*;
use nous_foundation::execution;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

pub(super) const FORMAT: &str = "nous-subject-package";
pub(super) struct Loaded {
    pub manifest: Value,
    pub package: SubjectPackage,
    pub digest: String,
    pub members: BTreeMap<String, Member>,
    pub object_members: BTreeMap<String, String>,
    pub unaccepted_vector_members: Vec<String>,
}
pub(super) fn digest(value: &impl serde::Serialize) -> Result<String> {
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}
pub(super) fn content_digest(document: &Value) -> Result<String> {
    let mut content = document.clone();
    let body = content
        .as_object_mut()
        .ok_or_else(|| Error::invalid("package content must be an object"))?;
    body.remove("created_at");
    body.remove("watermark");
    digest(&content)
}
pub(super) fn load(path: &Path, limits: &Limits) -> Result<Option<Loaded>> {
    let contents = bundle_format::read_archive(File::open(path)?, limits).map_err(format_error)?;
    if contents.json_documents["manifest.json"]
        .get("format")
        .and_then(Value::as_str)
        != Some(FORMAT)
    {
        return Ok(None);
    }
    let knowledge = &contents.json_documents["knowledge.json"];
    // Integrity belongs to the transmitted document; domain decoding may choose
    // a different JSON number spelling inside opaque provider payloads.
    let expected = content_digest(knowledge)?;
    if contents.json_documents["manifest.json"]
        .get("document_digest")
        .and_then(Value::as_str)
        != Some(digest(knowledge)?.as_str())
    {
        return Err(Error::invalid("subject package document digest mismatch"));
    }
    if contents.json_documents["manifest.json"]
        .get("content_digest")
        .and_then(Value::as_str)
        != Some(expected.as_str())
    {
        return Err(Error::invalid("subject package semantic digest mismatch"));
    }
    let package: SubjectPackage = serde_json::from_value(knowledge.clone())?;
    package.validate()?;
    let checksums: Checksums =
        serde_json::from_value(contents.json_documents["checksums.json"].clone())?;
    if checksums.algorithm != "sha256" {
        return Err(Error::invalid("bundle checksum algorithm must be sha256"));
    }
    let mut expected_names =
        BTreeSet::from(["manifest.json".to_owned(), "knowledge.json".to_owned()]);
    let mut object_members = BTreeMap::new();
    for blob in package.objects()? {
        let name = format!("objects/{}", blob.content_hash);
        let member = contents
            .members
            .get(&name)
            .ok_or_else(|| Error::invalid("bundle omits a referenced object"))?;
        if member.sha256 != blob.content_hash || member.byte_len != blob.byte_len {
            return Err(Error::invalid(
                "bundle object differs from its content reference",
            ));
        }
        expected_names.insert(name.clone());
        object_members.insert(blob.content_hash, name);
    }
    if checksums.files.keys().cloned().collect::<BTreeSet<_>>() != expected_names
        || contents.members.len() != expected_names.len() + 1
    {
        return Err(Error::invalid(
            "bundle checksum inventory does not cover the exact package",
        ));
    }
    for (name, hash) in checksums.files {
        if contents.members.get(&name).is_none_or(|m| m.sha256 != hash) {
            return Err(
                Error::invalid("bundle member checksum mismatch").with_context("member", name)
            );
        }
    }
    Ok(Some(Loaded {
        manifest: contents.json_documents["manifest.json"].clone(),
        package,
        digest: expected,
        members: contents.members,
        object_members,
        unaccepted_vector_members: vec![],
    }))
}
pub(super) fn write(
    root: &Path,
    package: &SubjectPackage,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    execution::check_cancelled(cancel)?;
    let mut knowledge = serde_json::to_value(package)?;
    knowledge.sort_all_objects();
    let document_digest = digest(&knowledge)?;
    let digest = content_digest(&knowledge)?;
    let knowledge = serde_json::to_vec(&knowledge)?;
    let manifest = serde_json::to_vec(
        &json!({"format":FORMAT,"package":package.package,"content_level":package.content_level,"content_digest":digest,"document_digest":document_digest}),
    )?;
    let mut checksums = BTreeMap::from([
        ("manifest.json".to_owned(), hash(&manifest)),
        ("knowledge.json".to_owned(), hash(&knowledge)),
    ]);
    for blob in package.objects()? {
        checksums.insert(format!("objects/{}", blob.content_hash), blob.content_hash);
    }
    let path = root.join("subject.nousbundle");
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(&path)?;
    let mut zip = ZipWriter::new(file);
    for (name, bytes) in [
        ("manifest.json", manifest),
        ("knowledge.json", knowledge),
        (
            "checksums.json",
            serde_json::to_vec(&json!({"algorithm":"sha256","files":checksums}))?,
        ),
    ] {
        zip.start_file(name, SimpleFileOptions::default())
            .map_err(zip_error)?;
        zip.write_all(&bytes)?;
    }
    for blob in package.objects()? {
        execution::check_cancelled(cancel)?;
        zip.start_file(
            format!("objects/{}", blob.content_hash),
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
        )
        .map_err(zip_error)?;
        copy(
            &mut File::open(root.join(&blob.content_hash))?,
            &mut zip,
            cancel,
        )?;
    }
    zip.finish().map_err(zip_error)?.sync_all()?;
    Ok(path)
}
pub(super) fn extract(
    path: &Path,
    root: &Path,
    members: &BTreeMap<String, Member>,
    names: &BTreeMap<String, String>,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut zip = ZipArchive::new(File::open(path)?).map_err(zip_error)?;
    for (hash, name) in names {
        execution::check_cancelled(cancel)?;
        let member = members
            .get(name)
            .ok_or_else(|| Error::invalid("object member not in validated inventory"))?;
        let mut input = zip.by_index(member.index).map_err(zip_error)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(root.join(hash))?;
        let copied = copy(&mut input, &mut output, cancel)?;
        if copied != member.byte_len {
            return Err(Error::invalid("archive changed after validation"));
        }
        output.sync_all()?;
    }
    Ok(())
}
fn copy(input: &mut impl Read, output: &mut impl Write, cancel: &CancellationToken) -> Result<u64> {
    let mut buffer = [0u8; 65536];
    let mut total = 0;
    loop {
        execution::check_cancelled(cancel)?;
        let n = input.read(&mut buffer)?;
        if n == 0 {
            return Ok(total);
        }
        output.write_all(&buffer[..n])?;
        total += n as u64;
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn format_error(error: anyhow::Error) -> Error {
    if let Some(domain) = error.downcast_ref::<Error>() {
        let mut mapped = domain.clone();
        if mapped.code == ErrorCode::ResourceExhausted {
            mapped = mapped.with_context("configuration_key", "bundles");
        }
        return mapped.with_cause(error.as_ref());
    }
    Error::invalid("invalid knowledge bundle").with_cause(error.as_ref())
}
fn zip_error(error: zip::result::ZipError) -> Error {
    Error::invalid("invalid knowledge ZIP").with_cause(&error)
}
