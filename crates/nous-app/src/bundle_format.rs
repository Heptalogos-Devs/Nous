//! A_Memorix v1/v2 archive inspection, without installation or filesystem extraction.
//!
//! Adapted from MaiBot/src/A_memorix/core/runtime/services/bundle_admin_service.py
//! at c602a2780f4f4dda4a6be46d67abc1cfb1ad420c: _load_bundle, member hashing,
//! _canonical_json_bytes, _canonical_image_semantics and RESOURCE_PRIMARY_KEYS.
//! SPDX-License-Identifier: AGPL-3.0-only. See licenses/A_memorix-LICENSE.
use anyhow::{Context, Result, ensure};
use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
};
use zip::{CompressionMethod, ZipArchive};

pub const FORMAT: &str = "a-memorix-memory-bundle";
pub const STATE_TABLES: &[(&str, &[&str])] = &[
    ("paragraphs", &["hash"]),
    ("entities", &["hash"]),
    ("relations", &["hash"]),
    ("deleted_relations", &["hash"]),
    ("paragraph_entities", &["paragraph_hash", "entity_hash"]),
    ("paragraph_relations", &["paragraph_hash", "relation_hash"]),
    ("external_memory_refs", &["external_id"]),
    ("episodes", &["episode_id"]),
    ("episode_paragraphs", &["episode_id", "paragraph_hash"]),
    ("fact_claims", &["claim_id"]),
    (
        "fact_evidence",
        &["claim_id", "evidence_type", "evidence_id", "stance"],
    ),
    ("fact_transitions", &["transition_id"]),
    ("person_profile_snapshots", &["snapshot_id"]),
    ("person_profile_overrides", &["person_id"]),
    ("person_profile_alias_overrides", &["person_id"]),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    pub archive_bytes: u64,
    pub members: usize,
    pub member_bytes: u64,
    pub expanded_bytes: u64,
    pub central_directory_bytes: u64,
    pub json_bytes: u64,
    pub json_nodes: usize,
    pub json_depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            archive_bytes: 1024 * 1024 * 1024,
            members: 4096,
            member_bytes: 256 * 1024 * 1024,
            expanded_bytes: 1024 * 1024 * 1024,
            central_directory_bytes: 4 * 1024 * 1024,
            json_bytes: 32 * 1024 * 1024,
            json_nodes: 250_000,
            json_depth: 64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub id: String,
    pub version: String,
    pub name: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentLevel {
    Knowledge,
    Full,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    pub content_level: ContentLevel,
    pub content_digest: String,
    pub package: Package,
    pub components: Vec<String>,
    pub counts: BTreeMap<String, u64>,
    #[serde(default)]
    pub embedding_fingerprint: Value,
    #[serde(default)]
    pub image_embedding_fingerprint: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checksums {
    pub algorithm: String,
    pub files: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDoc {
    pub id: String,
    pub passage: String,
    #[serde(default)]
    pub extracted_entities: Vec<String>,
    #[serde(default)]
    pub extracted_triples: Vec<[String; 3]>,
    #[serde(default)]
    pub a_memorix: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Knowledge {
    pub format: String,
    pub format_version: u32,
    pub docs: Vec<KnowledgeDoc>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub schema: String,
    pub schema_version: u32,
    pub tables: BTreeMap<String, Vec<Map<String, Value>>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAsset {
    pub asset_id: String,
    pub content_hash: String,
    pub member: String,
    pub byte_size: u64,
    pub mime_type: String,
    pub width: u64,
    pub height: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageOccurrence {
    pub occurrence_id: String,
    pub asset_id: String,
    #[serde(default)]
    pub component_path: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageObservation {
    pub occurrence_id: String,
    pub text: String,
    pub source_kind: String,
    pub confirm_status: String,
    #[serde(default)]
    pub evidence_json: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageLink {
    pub occurrence_id: String,
    pub target_type: String,
    pub target_id: String,
    pub link_kind: String,
    #[serde(default)]
    pub evidence_json: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Images {
    pub assets: Vec<ImageAsset>,
    pub occurrences: Vec<ImageOccurrence>,
    pub observations: Vec<ImageObservation>,
    pub links: Vec<ImageLink>,
    #[serde(default)]
    pub fingerprint: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberKind {
    Manifest,
    Checksums,
    Knowledge,
    State,
    ImageRecords,
    ImageAsset,
    ParagraphVectors,
    GraphVectors,
    ImageVectors,
    Extension,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub index: usize,
    pub kind: MemberKind,
    pub sha256: String,
    pub byte_len: u64,
    pub compressed_bytes: u64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReferenceIndex {
    pub documents: BTreeMap<String, usize>,
    pub knowledge_entities: BTreeSet<String>,
    pub knowledge_relations: BTreeSet<String>,
    pub state_rows: BTreeMap<String, BTreeMap<String, usize>>,
    pub image_assets: BTreeMap<String, usize>,
    pub image_occurrences: BTreeMap<String, usize>,
    pub vector_members: BTreeMap<String, String>,
    pub extension_members: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRequirement {
    pub capability: String,
    pub records: u64,
    pub members: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleInspection {
    pub manifest: Manifest,
    pub checksums: Checksums,
    pub knowledge: Knowledge,
    pub state: Option<State>,
    pub images: Option<Images>,
    pub members: BTreeMap<String, Member>,
    pub index: ReferenceIndex,
    pub required_capabilities: Vec<CapabilityRequirement>,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub expanded_bytes: u64,
}

pub fn inspect_file(path: &Path) -> Result<BundleInspection> {
    inspect(File::open(path)?, &Limits::default())
}
pub fn inspect_bytes(bytes: &[u8]) -> Result<BundleInspection> {
    inspect(Cursor::new(bytes), &Limits::default())
}

pub fn inspect<R: Read + Seek>(mut reader: R, limits: &Limits) -> Result<BundleInspection> {
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
    let manifest_raw = json_documents
        .remove("manifest.json")
        .context("missing manifest JSON")?;
    let manifest: Manifest =
        serde_json::from_value(manifest_raw).context("invalid bundle manifest schema")?;
    ensure!(
        manifest.format == FORMAT && matches!(manifest.format_version, 1 | 2),
        "unsupported bundle format/version"
    );
    for value in [
        &manifest.package.id,
        &manifest.package.version,
        &manifest.package.name,
    ] {
        ensure!(!value.trim().is_empty(), "invalid package identity");
    }
    ensure!(
        valid_hash(&manifest.content_digest),
        "invalid semantic content digest"
    );
    let declared_components: BTreeSet<_> = manifest.components.iter().cloned().collect();
    let actual_components: BTreeSet<_> = members
        .keys()
        .filter(|name| !matches!(name.as_str(), "manifest.json" | "checksums.json"))
        .cloned()
        .collect();
    ensure!(
        declared_components.len() == manifest.components.len()
            && declared_components == actual_components,
        "manifest components do not cover the archive"
    );
    let checksums: Checksums = serde_json::from_value(
        json_documents
            .remove("checksums.json")
            .context("missing checksums JSON")?,
    )
    .context("invalid checksums schema")?;
    ensure!(
        checksums.algorithm == "sha256",
        "unsupported bundle checksum algorithm"
    );
    let files = &checksums.files;
    ensure!(
        files.len() + 1 == members.len()
            && files
                .keys()
                .all(|name| name != "checksums.json" && members.contains_key(name)),
        "checksum coverage is incomplete"
    );
    for (name, expected) in files {
        ensure!(
            expected == &members[name].sha256,
            "bundle checksum mismatch: {name}"
        );
    }
    let knowledge_raw = json_documents
        .remove("knowledge.json")
        .context("missing knowledge JSON")?;
    let state_raw = json_documents.remove("state.json");
    let images_raw = json_documents.remove("images.json");
    ensure!(
        manifest.content_level != ContentLevel::Full || state_raw.is_some(),
        "full bundle requires state.json"
    );
    ensure!(
        manifest.format_version != 1 || images_raw.is_none(),
        "v1 bundle cannot contain image records"
    );
    ensure!(
        manifest.format_version != 2 || images_raw.is_some(),
        "v2 bundle requires images.json"
    );
    let knowledge: Knowledge =
        serde_json::from_value(knowledge_raw.clone()).context("invalid knowledge schema")?;
    ensure!(
        knowledge.format == "lpmm_openie" && knowledge.format_version == 1,
        "unsupported knowledge schema"
    );
    let state = state_raw
        .as_ref()
        .map(|state| serde_json::from_value::<State>(state.clone()))
        .transpose()
        .context("invalid full-state schema")?;
    if let Some(state) = &state {
        ensure!(
            state.schema == "a-memorix-semantic-state" && state.schema_version == 1,
            "unsupported semantic state schema"
        );
    }
    let images = images_raw
        .as_ref()
        .map(|images| serde_json::from_value::<Images>(images.clone()))
        .transpose()
        .context("invalid image record schema")?;
    let mut semantics = json!({"knowledge":knowledge_raw,"state":if manifest.content_level==ContentLevel::Full {state_raw.as_ref().unwrap_or(&Value::Null)}else{&Value::Null}});
    if let Some(images) = &images {
        semantics["images"] = canonical_images(images, &mut remaining_nodes, limits.json_depth)?;
    }
    ensure!(
        format!("{:x}", Sha256::digest(canonical_json(&semantics)?)) == manifest.content_digest,
        "bundle semantic content digest mismatch"
    );
    let index = build_index(&knowledge, state.as_ref(), images.as_ref(), &members)?;
    validate_counts(&manifest, &knowledge, state.as_ref(), images.as_ref())?;
    let required_capabilities = requirements(&knowledge, state.as_ref(), images.as_ref(), &index);
    Ok(BundleInspection {
        manifest,
        checksums,
        knowledge,
        state,
        images,
        members,
        index,
        required_capabilities,
        archive_sha256,
        archive_bytes,
        expanded_bytes,
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
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

struct JsonSeed<'a> {
    remaining: &'a mut usize,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for JsonSeed<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        if *self.remaining == 0 || self.depth == 0 {
            return Err(de::Error::custom("bundle JSON node/depth limit exceeded"));
        }
        *self.remaining -= 1;
        deserializer.deserialize_any(JsonVisitor {
            remaining: self.remaining,
            depth: self.depth - 1,
        })
    }
}
struct JsonVisitor<'a> {
    remaining: &'a mut usize,
    depth: usize,
}
impl<'de> Visitor<'de> for JsonVisitor<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("bounded JSON value")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Value, E> {
        Ok(json!(value))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Value, E> {
        Ok(json!(value))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite JSON number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<Value, A::Error> {
        let mut result = Vec::new();
        while let Some(value) = sequence.next_element_seed(JsonSeed {
            remaining: self.remaining,
            depth: self.depth,
        })? {
            result.push(value);
        }
        Ok(Value::Array(result))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> std::result::Result<Value, A::Error> {
        let mut result = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if result.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            let value = object.next_value_seed(JsonSeed {
                remaining: self.remaining,
                depth: self.depth,
            })?;
            result.insert(key, value);
        }
        Ok(Value::Object(result))
    }
}
fn bounded_json(bytes: &[u8], remaining: &mut usize, depth: usize) -> Result<Value> {
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let value = JsonSeed { remaining, depth }.deserialize(&mut parser)?;
    parser.end()?;
    Ok(value)
}

/// Python json.dumps(sort_keys=True,separators=(',',':'),ensure_ascii=False) compatibility.
fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    let mut text = String::new();
    canonical_into(value, &mut text)?;
    Ok(text.into_bytes())
}
fn canonical_into(value: &Value, output: &mut String) -> Result<()> {
    match value {
        Value::Object(fields) => {
            output.push('{');
            let mut keys: Vec<_> = fields.keys().collect();
            keys.sort();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key)?);
                output.push(':');
                canonical_into(&fields[key], output)?;
            }
            output.push('}');
        }
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                canonical_into(value, output)?;
            }
            output.push(']');
        }
        Value::Number(number) if number.is_f64() => output.push_str(&python_float(number)),
        _ => output.push_str(&serde_json::to_string(value)?),
    }
    Ok(())
}
fn python_float(number: &Number) -> String {
    let raw = number.to_string();
    let (sign, raw) = if let Some(raw) = raw.strip_prefix('-') {
        ("-", raw)
    } else {
        ("", raw.as_str())
    };
    let (mantissa, exponent) = raw
        .split_once(['e', 'E'])
        .map(|(mantissa, exponent)| {
            (
                mantissa,
                exponent.parse::<i32>().expect("JSON number exponent"),
            )
        })
        .unwrap_or((raw, 0));
    let mut point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let mut digits = mantissa.replace('.', "");
    let leading = digits.bytes().take_while(|byte| *byte == b'0').count();
    digits.drain(..leading);
    point -= leading as i32;
    while digits.ends_with('0') {
        digits.pop();
    }
    if digits.is_empty() {
        return format!("{sign}0.0");
    }
    let power = point - 1;
    if !(-4..16).contains(&power) {
        let mut mantissa = digits[..1].to_owned();
        if digits.len() > 1 {
            mantissa.push('.');
            mantissa.push_str(&digits[1..]);
        }
        return format!(
            "{sign}{mantissa}e{}{power_abs:02}",
            if power < 0 { '-' } else { '+' },
            power_abs = power.unsigned_abs()
        );
    }
    if point <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
    } else if point as usize >= digits.len() {
        format!(
            "{sign}{digits}{}.0",
            "0".repeat(point as usize - digits.len())
        )
    } else {
        format!(
            "{sign}{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    }
}

fn evidence(value: &Value, remaining: &mut usize, depth: usize) -> Result<Value> {
    let mut object = match value {
        Value::Object(value) => value.clone(),
        Value::String(text) if !text.trim().is_empty() => {
            match bounded_json(text.as_bytes(), remaining, depth) {
                Ok(Value::Object(object)) => object,
                Ok(_) => Map::new(),
                // The upstream digest treats legacy malformed evidence as {} while
                // retaining the original evidence_json in the portable record.
                Err(error)
                    if error
                        .downcast_ref::<serde_json::Error>()
                        .is_some_and(|error| error.is_syntax() || error.is_eof()) =>
                {
                    Map::new()
                }
                Err(error) => return Err(error),
            }
        }
        _ => Map::new(),
    };
    for key in [
        "chat_id",
        "external_ref",
        "imported",
        "message_id",
        "source_link_id",
    ] {
        object.remove(key);
    }
    Ok(Value::Object(object))
}
fn canonical_images(images: &Images, remaining: &mut usize, depth: usize) -> Result<Value> {
    let assets: BTreeMap<_, _> = images
        .assets
        .iter()
        .map(|asset| (asset.asset_id.as_str(), asset))
        .collect();
    let mut occurrences: Vec<_> = images.occurrences.iter().collect();
    occurrences.sort_by_key(|item| {
        (
            assets
                .get(item.asset_id.as_str())
                .map(|asset| asset.content_hash.as_str())
                .unwrap_or(""),
            item.component_path.as_str(),
            item.occurrence_id.as_str(),
        )
    });
    let keys: BTreeMap<_, _> = occurrences
        .iter()
        .enumerate()
        .map(|(index, item)| (item.occurrence_id.as_str(), format!("occurrence-{index}")))
        .collect();
    let mut asset_values:Vec<_>=images.assets.iter().map(|asset|json!({"byte_size":asset.byte_size,"content_hash":asset.content_hash,"height":asset.height,"mime_type":asset.mime_type,"width":asset.width})).collect();
    asset_values.sort_by(|a, b| a["content_hash"].as_str().cmp(&b["content_hash"].as_str()));
    let occurrence_values:Vec<_>=occurrences.iter().map(|item|json!({"asset_hash":assets.get(item.asset_id.as_str()).map(|asset|asset.content_hash.as_str()).unwrap_or(""),"component_path":item.component_path})).collect();
    let mut observations=images.observations.iter().map(|item|Ok(json!({"confirm_status":item.confirm_status,"occurrence":keys.get(item.occurrence_id.as_str()).map(String::as_str).unwrap_or(""),"source_kind":item.source_kind,"text":item.text,"evidence":evidence(&item.evidence_json,remaining,depth)?}))).collect::<Result<Vec<Value>>>()?;
    observations.sort_by(|a, b| {
        (
            a["occurrence"].as_str(),
            a["source_kind"].as_str(),
            a["text"].as_str(),
        )
            .cmp(&(
                b["occurrence"].as_str(),
                b["source_kind"].as_str(),
                b["text"].as_str(),
            ))
    });
    let mut links=images.links.iter().map(|item|Ok(json!({"link_kind":item.link_kind,"occurrence":keys.get(item.occurrence_id.as_str()).map(String::as_str).unwrap_or(""),"target_id":item.target_id,"target_type":item.target_type,"evidence":evidence(&item.evidence_json,remaining,depth)?}))).collect::<Result<Vec<Value>>>()?;
    links.sort_by(|a, b| {
        (
            a["occurrence"].as_str(),
            a["target_type"].as_str(),
            a["target_id"].as_str(),
        )
            .cmp(&(
                b["occurrence"].as_str(),
                b["target_type"].as_str(),
                b["target_id"].as_str(),
            ))
    });
    Ok(
        json!({"assets":asset_values,"occurrences":occurrence_values,"observations":observations,"links":links}),
    )
}

fn build_index(
    knowledge: &Knowledge,
    state: Option<&State>,
    images: Option<&Images>,
    members: &BTreeMap<String, Member>,
) -> Result<ReferenceIndex> {
    let mut index = ReferenceIndex::default();
    for (position, doc) in knowledge.docs.iter().enumerate() {
        ensure!(
            !doc.id.trim().is_empty() && !doc.passage.trim().is_empty(),
            "knowledge document identity and passage are required"
        );
        ensure!(
            index.documents.insert(doc.id.clone(), position).is_none(),
            "duplicate knowledge document ID"
        );
        for entity in doc.extracted_entities.iter().chain(
            doc.extracted_triples
                .iter()
                .flat_map(|triple| [&triple[0], &triple[2]]),
        ) {
            ensure!(!entity.trim().is_empty(), "empty extracted entity");
            index.knowledge_entities.insert(format!(
                "{:x}",
                Sha256::digest(entity.trim().to_lowercase().as_bytes())
            ));
        }
        for triple in &doc.extracted_triples {
            ensure!(
                triple.iter().all(|part| !part.trim().is_empty()),
                "empty extracted triple component"
            );
            let key = triple
                .iter()
                .map(|part| part.trim().to_lowercase())
                .collect::<Vec<_>>()
                .join("|");
            index
                .knowledge_relations
                .insert(format!("{:x}", Sha256::digest(key.as_bytes())));
        }
    }
    if let Some(state) = state {
        for (table, rows) in &state.tables {
            let keys = STATE_TABLES
                .iter()
                .find(|(name, _)| name == table)
                .map(|(_, keys)| *keys);
            let mut row_index = BTreeMap::new();
            for (position, row) in rows.iter().enumerate() {
                let identity = if let Some(keys) = keys {
                    Value::Array(
                        keys.iter()
                            .map(|key| {
                                row.get(*key)
                                    .cloned()
                                    .context("state row lacks primary key")
                            })
                            .collect::<Result<_>>()?,
                    )
                } else {
                    Value::Object(row.clone())
                };
                let key = String::from_utf8(canonical_json(&identity)?)?;
                ensure!(
                    row_index.insert(key, position).is_none(),
                    "duplicate semantic state row identity"
                );
            }
            index.state_rows.insert(table.clone(), row_index);
        }
    }
    validate_state_refs(state, &index)?;
    if let Some(images) = images {
        let mut declared = BTreeSet::new();
        for (position, asset) in images.assets.iter().enumerate() {
            ensure!(
                valid_hash(&asset.content_hash) && asset.member.starts_with("images/assets/"),
                "invalid image asset reference"
            );
            let member = members
                .get(&asset.member)
                .context("image asset member is missing")?;
            ensure!(
                member.sha256 == asset.content_hash && member.byte_len == asset.byte_size,
                "image asset hash/size mismatch"
            );
            ensure!(
                declared.insert(asset.member.clone())
                    && index
                        .image_assets
                        .insert(asset.asset_id.clone(), position)
                        .is_none(),
                "duplicate image asset identity/member"
            );
        }
        let actual: BTreeSet<_> = members
            .keys()
            .filter(|name| name.starts_with("images/assets/"))
            .cloned()
            .collect();
        ensure!(actual == declared, "image asset membership is not closed");
        for (position, occurrence) in images.occurrences.iter().enumerate() {
            ensure!(
                index.image_assets.contains_key(&occurrence.asset_id),
                "image occurrence references unknown asset"
            );
            ensure!(
                index
                    .image_occurrences
                    .insert(occurrence.occurrence_id.clone(), position)
                    .is_none(),
                "duplicate image occurrence ID"
            );
        }
        for observation in &images.observations {
            ensure!(
                index
                    .image_occurrences
                    .contains_key(&observation.occurrence_id),
                "image observation references unknown occurrence"
            );
        }
        for link in &images.links {
            ensure!(
                index.image_occurrences.contains_key(&link.occurrence_id),
                "image link references unknown occurrence"
            );
            ensure!(
                target_exists(&link.target_type, &link.target_id, &index),
                "image link references a target outside the bundle"
            );
        }
    } else {
        ensure!(
            !members
                .keys()
                .any(|name| name.starts_with("images/assets/")),
            "image assets lack image records"
        );
    }
    for (name, member) in members {
        match member.kind {
            MemberKind::ParagraphVectors => {
                index
                    .vector_members
                    .insert("paragraph".into(), name.clone());
            }
            MemberKind::GraphVectors => {
                index.vector_members.insert("graph".into(), name.clone());
            }
            MemberKind::ImageVectors => {
                ensure!(images.is_some(), "image vectors lack image records");
                index.vector_members.insert("image".into(), name.clone());
            }
            MemberKind::Extension => index.extension_members.push(name.clone()),
            _ => {}
        }
    }
    Ok(index)
}
fn target_exists(kind: &str, id: &str, index: &ReferenceIndex) -> bool {
    if kind == "paragraph" && index.documents.contains_key(id) {
        return true;
    }
    if kind == "entity" && index.knowledge_entities.contains(id)
        || kind == "relation" && index.knowledge_relations.contains(id)
    {
        return true;
    }
    let table = match kind {
        "paragraph" => "paragraphs",
        "entity" => "entities",
        "relation" => "relations",
        "episode" => "episodes",
        _ => return false,
    };
    let key = format!(
        "[{}]",
        serde_json::to_string(id).expect("string is JSON serializable")
    );
    index
        .state_rows
        .get(table)
        .is_some_and(|rows| rows.contains_key(&key))
}
fn validate_state_refs(state: Option<&State>, index: &ReferenceIndex) -> Result<()> {
    let Some(state) = state else {
        return Ok(());
    };
    for (table, rows) in &state.tables {
        for row in rows {
            for (field, kind) in match table.as_str() {
                "paragraph_entities" => {
                    &[("paragraph_hash", "paragraph"), ("entity_hash", "entity")][..]
                }
                "paragraph_relations" => &[
                    ("paragraph_hash", "paragraph"),
                    ("relation_hash", "relation"),
                ][..],
                "episode_paragraphs" => {
                    &[("episode_id", "episode"), ("paragraph_hash", "paragraph")][..]
                }
                "external_memory_refs" => &[("paragraph_hash", "paragraph")][..],
                _ => &[][..],
            } {
                let id = row
                    .get(*field)
                    .and_then(Value::as_str)
                    .context("invalid semantic reference")?;
                ensure!(
                    target_exists(kind, id, index),
                    "semantic state references a missing {kind}: {id}"
                );
            }
        }
    }
    Ok(())
}
fn validate_counts(
    manifest: &Manifest,
    knowledge: &Knowledge,
    state: Option<&State>,
    images: Option<&Images>,
) -> Result<()> {
    if let Some(count) = manifest.counts.get("paragraphs") {
        ensure!(
            *count == knowledge.docs.len() as u64,
            "paragraph count mismatch"
        );
    }
    if manifest.content_level == ContentLevel::Full
        && let Some(state) = state
    {
        for (name, rows) in &state.tables {
            if let Some(count) = manifest.counts.get(name) {
                ensure!(
                    *count == rows.len() as u64,
                    "semantic state count mismatch: {name}"
                );
            }
        }
    }
    if let Some(images) = images {
        for (name, count) in [
            ("image_assets", images.assets.len()),
            ("image_occurrences", images.occurrences.len()),
            ("image_observations", images.observations.len()),
            ("image_links", images.links.len()),
        ] {
            if let Some(expected) = manifest.counts.get(name) {
                ensure!(*expected == count as u64, "image count mismatch");
            }
        }
    }
    Ok(())
}
fn requirements(
    knowledge: &Knowledge,
    state: Option<&State>,
    images: Option<&Images>,
    index: &ReferenceIndex,
) -> Vec<CapabilityRequirement> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    counts.insert("package_installation".into(), 1);
    counts.insert("material_memory".into(), knowledge.docs.len() as u64);
    let graph = knowledge
        .docs
        .iter()
        .map(|doc| doc.extracted_entities.len() + doc.extracted_triples.len())
        .sum::<usize>();
    if graph > 0 {
        counts.insert("graph".into(), graph as u64);
    }
    if let Some(state) = state {
        for (name, rows) in &state.tables {
            if rows.is_empty() {
                continue;
            }
            let capability = match name.as_str() {
                "paragraphs" | "external_memory_refs" => "material_memory",
                "entities"
                | "relations"
                | "deleted_relations"
                | "paragraph_entities"
                | "paragraph_relations" => "graph",
                "episodes" | "episode_paragraphs" => "episodes",
                "fact_claims" | "fact_evidence" | "fact_transitions" => "facts",
                "person_profile_snapshots"
                | "person_profile_overrides"
                | "person_profile_alias_overrides" => "profiles",
                _ => "state_extensions",
            };
            *counts.entry(capability.into()).or_default() += rows.len() as u64;
        }
    }
    if let Some(images) = images {
        counts.insert("images".into(), images.assets.len() as u64);
    }
    let mut result: Vec<_> = counts
        .into_iter()
        .map(|(capability, records)| CapabilityRequirement {
            capability,
            records,
            members: Vec::new(),
        })
        .collect();
    for (kind, member) in &index.vector_members {
        result.push(CapabilityRequirement {
            capability: if kind == "image" {
                "image_embeddings"
            } else {
                "dense_vectors"
            }
            .into(),
            records: 1,
            members: vec![member.clone()],
        });
    }
    if !index.extension_members.is_empty() {
        result.push(CapabilityRequirement {
            capability: "archive_extensions".into(),
            records: index.extension_members.len() as u64,
            members: index.extension_members.clone(),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::SimpleFileOptions};

    const KNOWLEDGE: &[u8] =
        include_bytes!("../tests/fixtures/bundles/v1-knowledge-1.0.0.amembundle");
    const FULL: &[u8] = include_bytes!("../tests/fixtures/bundles/v1-full-1.0.0.amembundle");
    const IMAGE: &[u8] = include_bytes!("../tests/fixtures/bundles/v2-images-1.0.0.amembundle");

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

    #[test]
    fn python_canonical_numbers_unicode_and_encoded_evidence_match() -> Result<()> {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../tests/fixtures/bundles/canonical-cases.json"
        ))?;
        for case in cases {
            let input = case["input"].as_str().context("missing canonical input")?;
            let value = bounded_json(input.as_bytes(), &mut 1000, 64)?;
            assert_eq!(
                String::from_utf8(canonical_json(&value)?)?,
                case["canonical"].as_str().unwrap(),
                "Python mismatch for {input}"
            );
        }
        let encoded = json!(
            "{\"chat_id\":\"old\",\"message_id\":\"old\",\"confidence\":1e-7,\"中文\":\"🧠\"}"
        );
        assert_eq!(
            evidence(&encoded, &mut 100, 64)?,
            json!({"confidence":1e-7,"中文":"🧠"})
        );
        assert_eq!(
            evidence(&json!("legacy: not-json"), &mut 100, 64)?,
            json!({})
        );
        assert_eq!(evidence(&json!("[1,2]"), &mut 100, 64)?, json!({}));
        assert!(evidence(&json!("{\"a\":[1,2]}"), &mut 1, 64).is_err());
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
        members.insert("manifest.json".into(), canonical_json(&manifest)?);
        let files: BTreeMap<_, _> = members
            .iter()
            .filter(|(name, _)| name.as_str() != "checksums.json")
            .map(|(name, bytes)| (name, format!("{:x}", Sha256::digest(bytes))))
            .collect();
        let checksums = canonical_json(&json!({"algorithm":"sha256","files":files}))?;
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
        members.insert("checksums.json".into(), canonical_json(&checksums)?);
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

        let mut members = unpack(IMAGE)?;
        let mut image_records: Value = serde_json::from_slice(&members["images.json"])?;
        image_records["occurrences"][0]["asset_id"] = json!("missing-asset");
        let images: Images = serde_json::from_value(image_records.clone())?;
        let semantics = json!({"knowledge":serde_json::from_slice::<Value>(&members["knowledge.json"])? ,"state":serde_json::from_slice::<Value>(&members["state.json"])? ,"images":canonical_images(&images,&mut 10000,64)?});
        let mut manifest: Value = serde_json::from_slice(&members["manifest.json"])?;
        manifest["content_digest"] =
            json!(format!("{:x}", Sha256::digest(canonical_json(&semantics)?)));
        members.insert("manifest.json".into(), canonical_json(&manifest)?);
        members.insert("images.json".into(), canonical_json(&image_records)?);
        repair_checksums(&mut members)?;
        assert!(
            inspect_bytes(&pack(&members)?)
                .unwrap_err()
                .to_string()
                .contains("unknown asset")
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
        assert!(bounded_json(br#"{"x":1,"x":2}"#, &mut 100, 64).is_err());
        Ok(())
    }
}
