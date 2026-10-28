use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
            json_nodes: 2_000_000,
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

pub(crate) fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
