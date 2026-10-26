use super::{Limits, Member, MemberKind, json::bounded_json};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom},
};
use zip::{CompressionMethod, ZipArchive};
pub(crate) struct ArchiveContents {
    pub members: BTreeMap<String, Member>,
    pub json_documents: BTreeMap<String, Value>,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub expanded_bytes: u64,
    pub remaining_nodes: usize,
}
pub(crate) fn read_archive<R: Read + Seek>(
    mut reader: R,
    limits: &Limits,
) -> Result<ArchiveContents> {
    let archive_bytes = reader.seek(SeekFrom::End(0))?;
    ensure!(
        archive_bytes <= limits.archive_bytes,
        "bundle archive exceeds configured byte limit"
    );
    let declared = preflight(&mut reader, archive_bytes, limits)?;
    reader.seek(SeekFrom::Start(0))?;
    let archive_sha256 = hash_reader(&mut reader, archive_bytes)?.0;
    reader.seek(SeekFrom::Start(0))?;
    let mut archive = ZipArchive::new(reader).context("invalid bundle ZIP")?;
    ensure!(
        archive.len() == declared && archive.len() <= limits.members,
        "duplicate ZIP entries or inconsistent directory count"
    );
    let mut members = BTreeMap::new();
    let mut folded = BTreeSet::new();
    let mut expanded_bytes = 0u64;
    let mut json_size = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index_data(index)?;
        let name = entry.name()?.into_owned();
        safe_name(&name)?;
        ensure!(
            entry.is_file() && !entry.encrypted(),
            "bundle members must be unencrypted regular files"
        );
        ensure!(
            matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            ),
            "unsupported bundle compression method"
        );
        ensure!(
            entry.size() <= limits.member_bytes,
            "bundle member exceeds byte limit: {name}"
        );
        expanded_bytes = expanded_bytes
            .checked_add(entry.size())
            .context("bundle size overflow")?;
        ensure!(
            expanded_bytes <= limits.expanded_bytes,
            "bundle exceeds expanded byte limit"
        );
        if is_json(&name) {
            json_size = json_size
                .checked_add(entry.size())
                .context("JSON size overflow")?;
            ensure!(
                json_size <= limits.json_bytes,
                "bundle JSON exceeds byte limit"
            );
        }
        ensure!(
            folded.insert(name.to_lowercase()),
            "case-colliding ZIP entries: {name}"
        );
        ensure!(
            members
                .insert(
                    name.clone(),
                    Member {
                        index,
                        kind: kind(&name),
                        sha256: String::new(),
                        byte_len: entry.size(),
                        compressed_bytes: entry.compressed_size()
                    }
                )
                .is_none(),
            "duplicate ZIP member: {name}"
        );
    }
    for required in ["manifest.json", "knowledge.json", "checksums.json"] {
        ensure!(members.contains_key(required), "bundle lacks {required}");
    }
    let mut json_documents = BTreeMap::new();
    let mut remaining_nodes = limits.json_nodes;
    for (name, member) in &mut members {
        let mut file = archive.by_index(member.index)?;
        let mut bytes = if is_json(name) {
            Some(Vec::with_capacity(member.byte_len as usize))
        } else {
            None
        };
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .context("member size overflow")?;
            ensure!(
                total <= member.byte_len && total <= limits.member_bytes,
                "ZIP member expanded beyond declared length"
            );
            hasher.update(&buffer[..count]);
            if let Some(bytes) = &mut bytes {
                bytes.extend_from_slice(&buffer[..count]);
            }
        }
        ensure!(
            total == member.byte_len,
            "ZIP member length mismatch: {name}"
        );
        member.sha256 = format!("{:x}", hasher.finalize());
        if let Some(bytes) = bytes {
            json_documents.insert(
                name.clone(),
                bounded_json(&bytes, &mut remaining_nodes, limits.json_depth)
                    .with_context(|| format!("invalid bundle JSON: {name}"))?,
            );
        }
    }
    Ok(ArchiveContents {
        members,
        json_documents,
        archive_sha256,
        archive_bytes,
        expanded_bytes,
        remaining_nodes,
    })
}

fn safe_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 512
            && !name.contains(['\\', ':', '\0'])
            && !name.starts_with('/')
            && name.split('/').all(|part| !matches!(part, "" | "." | "..")),
        "unsafe bundle member path: {name}"
    );
    Ok(())
}
fn is_json(name: &str) -> bool {
    matches!(
        name,
        "manifest.json" | "knowledge.json" | "checksums.json" | "state.json" | "images.json"
    )
}
fn kind(name: &str) -> MemberKind {
    match name {
        "manifest.json" => MemberKind::Manifest,
        "checksums.json" => MemberKind::Checksums,
        "knowledge.json" => MemberKind::Knowledge,
        "state.json" => MemberKind::State,
        "images.json" => MemberKind::ImageRecords,
        "vectors/paragraphs.npz" => MemberKind::ParagraphVectors,
        "vectors/graph.npz" => MemberKind::GraphVectors,
        "vectors/images.npz" => MemberKind::ImageVectors,
        name if name.starts_with("images/assets/") => MemberKind::ImageAsset,
        _ => MemberKind::Extension,
    }
}
fn hash_reader(reader: &mut impl Read, limit: u64) -> Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= limit, "archive changed while reading");
        hasher.update(&buffer[..count]);
    }
    ensure!(total == limit, "archive length changed while reading");
    Ok((format!("{:x}", hasher.finalize()), total))
}

/// Bound directory allocation before handing ZIP parsing to zip-rs. ZIP64 is supported.
fn preflight(reader: &mut (impl Read + Seek), length: u64, limits: &Limits) -> Result<usize> {
    ensure!(length >= 22, "truncated bundle ZIP");
    let tail_len = length.min(65_557) as usize;
    reader.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0; tail_len];
    reader.read_exact(&mut tail)?;
    let position = (0..=tail_len - 22)
        .rev()
        .find(|position| {
            tail[*position..].starts_with(b"PK\x05\x06")
                && *position
                    + 22
                    + u16::from_le_bytes([tail[*position + 20], tail[*position + 21]]) as usize
                    == tail_len
        })
        .context("ZIP footer not found")?;
    let footer = &tail[position..];
    let u16_at = |offset| u16::from_le_bytes([footer[offset], footer[offset + 1]]);
    let u32_at = |offset| {
        u32::from_le_bytes(
            footer[offset..offset + 4]
                .try_into()
                .expect("fixed footer field"),
        )
    };
    ensure!(
        u16_at(4) == 0 && u16_at(6) == 0,
        "split ZIP archives are unsupported"
    );
    let mut count = u64::from(u16_at(10));
    let mut directory = u64::from(u32_at(12));
    let mut directory_offset = u64::from(u32_at(16));
    let footer_position = length - tail_len as u64 + position as u64;
    let mut directory_end = footer_position;
    if count == u64::from(u16::MAX)
        || directory == u64::from(u32::MAX)
        || directory_offset == u64::from(u32::MAX)
    {
        ensure!(footer_position >= 20, "missing ZIP64 locator");
        reader.seek(SeekFrom::Start(footer_position - 20))?;
        let mut locator = [0; 20];
        reader.read_exact(&mut locator)?;
        ensure!(&locator[..4] == b"PK\x06\x07", "missing ZIP64 locator");
        let offset = u64::from_le_bytes(locator[8..16].try_into()?);
        ensure!(
            offset <= length.saturating_sub(56),
            "invalid ZIP64 footer offset"
        );
        reader.seek(SeekFrom::Start(offset))?;
        let mut zip64 = [0; 56];
        reader.read_exact(&mut zip64)?;
        ensure!(
            &zip64[..4] == b"PK\x06\x06" && zip64[16..24] == [0; 8],
            "invalid or split ZIP64 archive"
        );
        count = u64::from_le_bytes(zip64[32..40].try_into()?);
        directory = u64::from_le_bytes(zip64[40..48].try_into()?);
        directory_offset = u64::from_le_bytes(zip64[48..56].try_into()?);
        directory_end = offset;
    }
    ensure!(
        count > 0 && count <= limits.members as u64,
        "bundle ZIP member count exceeds limit"
    );
    ensure!(
        directory <= limits.central_directory_bytes,
        "bundle ZIP central directory exceeds limit"
    );
    ensure!(
        directory_offset
            .checked_add(directory)
            .is_some_and(|end| end <= directory_end),
        "invalid ZIP central directory range"
    );
    // Verify the advertised directory boundary before zip-rs allocates member
    // metadata. A forged small directory length must not hide large extra fields.
    reader.seek(SeekFrom::Start(directory_offset))?;
    let mut directory_bytes = vec![0; usize::try_from(directory)?];
    reader.read_exact(&mut directory_bytes)?;
    let mut remaining = directory_bytes.as_slice();
    for _ in 0..count {
        ensure!(
            remaining.len() >= 46 && remaining.starts_with(b"PK\x01\x02"),
            "invalid ZIP central directory record"
        );
        let field_len = |offset| {
            usize::from(u16::from_le_bytes([
                remaining[offset],
                remaining[offset + 1],
            ]))
        };
        let size = 46 + field_len(28) + field_len(30) + field_len(32);
        ensure!(
            size <= remaining.len(),
            "ZIP directory record exceeds declared boundary"
        );
        remaining = &remaining[size..];
    }
    ensure!(
        remaining.is_empty()
            || (remaining.len() >= 6
                && remaining.starts_with(b"PK\x05\x05")
                && usize::from(u16::from_le_bytes([remaining[4], remaining[5]])) + 6
                    == remaining.len()),
        "unexpected records in ZIP central directory"
    );
    Ok(count as usize)
}
