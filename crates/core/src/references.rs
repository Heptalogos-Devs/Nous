// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;

// Declare each reference once: its native type, wire kind, UUID access and formatting agree.
macro_rules! cognitive_refs {
    ($($variant:ident($id:ident) = $kind:literal),*; $($opaque:ident($oid:ident) = $okind:literal),*) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(tag = "kind", content = "id")]
        pub enum CognitiveRef {
            $(#[serde(rename = $kind)] $variant($id),)*
            $(#[serde(rename = $okind)] $opaque($oid),)*
        }
        impl CognitiveRef {
            pub fn kind(&self) -> &'static str {
                match self { $(Self::$variant(_) => $kind,)* $(Self::$opaque(_) => $okind,)* }
            }
            pub fn uuid(&self) -> Option<Uuid> {
                match self { $(Self::$variant(id) => Some(id.0),)* $(Self::$opaque(_) => None,)* }
            }
        }
        pub fn reference_parts(reference: &CognitiveRef) -> (String, String) {
            let value = match reference {
                $(CognitiveRef::$variant(id) => id.0.to_string(),)*
                $(CognitiveRef::$opaque(id) => id.as_str().to_owned(),)*
            };
            (reference.kind().to_owned(), value)
        }
        pub fn parse_reference(kind: &str, value: &str) -> Result<CognitiveRef> {
            Ok(match kind {
                $($kind => CognitiveRef::$variant($id(value.parse()
                    .map_err(|_| Error::Invalid(format!("invalid {kind} reference")))?)),)*
                $($okind => CognitiveRef::$opaque($oid::new(value)?),)*
                _ => return Err(Error::Invalid("unknown reference kind".into())),
            })
        }
        impl std::fmt::Display for CognitiveRef {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(Self::$variant(id) => write!(f, "{}:{}", $kind, id.0),)*
                    $(Self::$opaque(id) => f.write_str(id.as_str()),)*
                }
            }
        }
    };
}

cognitive_refs! {
    SelfFacet(SelfFacetId) = "self_facet",
    SelfFacetRevision(SelfFacetRevisionId) = "self_facet_revision",
    NarrativeThread(NarrativeThreadId) = "narrative_thread",
    NarrativeThreadRevision(NarrativeThreadRevisionId) = "narrative_thread_revision",
    Memory(MemoryId) = "memory",
    MemoryRevision(MemoryRevisionId) = "memory_revision",
    Episode(EpisodeId) = "episode",
    EpisodeRevision(EpisodeRevisionId) = "episode_revision",
    Journal(JournalId) = "journal",
    JournalRevision(JournalRevisionId) = "journal_revision",
    CognitiveSchema(CognitiveSchemaId) = "cognitive_schema",
    CognitiveSchemaRevision(CognitiveSchemaRevisionId) = "cognitive_schema_revision",
    CognitiveSeedVersion(CognitiveSeedVersionId) = "cognitive_seed_version",
    Artifact(ArtifactId) = "artifact",
    SourceRegion(SourceRegionId) = "source_region",
    DerivedRepresentation(DerivedRepresentationId) = "derived_representation",
    DerivedRegion(DerivedRegionId) = "derived_region",
    Tag(TagId) = "tag",
    Occurrence(OccurrenceId) = "occurrence",
    Session(SessionId) = "session",
    Subject(SubjectId) = "subject",
    WorkContext(WorkContextId) = "work_context",
    Association(AssociationEvidenceId) = "association";
    Entity(EntityRef) = "entity",
    Resource(ResourceRef) = "resource",
    ExternalObject(ObjectRef) = "external_object"
}

/// Semantic ownership is shared by consumers; wire spelling does not determine policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitiveDomain {
    Memory,
    SelfCognition,
    Runtime,
    Material,
    Resource,
}
impl CognitiveRef {
    pub fn domain(&self) -> CognitiveDomain {
        match self {
            Self::Memory(_)
            | Self::MemoryRevision(_)
            | Self::CognitiveSchema(_)
            | Self::CognitiveSchemaRevision(_)
            | Self::Episode(_)
            | Self::EpisodeRevision(_)
            | Self::Journal(_)
            | Self::JournalRevision(_)
            | Self::Tag(_) => CognitiveDomain::Memory,
            Self::SelfFacet(_)
            | Self::SelfFacetRevision(_)
            | Self::NarrativeThread(_)
            | Self::NarrativeThreadRevision(_) => CognitiveDomain::SelfCognition,
            Self::Artifact(_)
            | Self::Occurrence(_)
            | Self::SourceRegion(_)
            | Self::DerivedRegion(_)
            | Self::DerivedRepresentation(_)
            | Self::ExternalObject(_) => CognitiveDomain::Material,
            Self::Resource(_) => CognitiveDomain::Resource,
            _ => CognitiveDomain::Runtime,
        }
    }
    pub fn is_cognition_revision(&self) -> bool {
        matches!(
            self,
            Self::MemoryRevision(_)
                | Self::CognitiveSchemaRevision(_)
                | Self::EpisodeRevision(_)
                | Self::JournalRevision(_)
                | Self::SelfFacetRevision(_)
                | Self::NarrativeThreadRevision(_)
        )
    }
}
