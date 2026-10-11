// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;

pub fn reference_parts(reference: &CognitiveRef) -> (String, String) {
    match reference {
        CognitiveRef::Memory(id) => ("memory".into(), id.0.to_string()),
        CognitiveRef::MemoryRevision(id) => ("memory_revision".into(), id.0.to_string()),
        CognitiveRef::Episode(id) => ("episode".into(), id.0.to_string()),
        CognitiveRef::EpisodeRevision(id) => ("episode_revision".into(), id.0.to_string()),
        CognitiveRef::Journal(id) => ("journal".into(), id.0.to_string()),
        CognitiveRef::JournalRevision(id) => ("journal_revision".into(), id.0.to_string()),
        CognitiveRef::CognitiveSchema(id) => ("cognitive_schema".into(), id.0.to_string()),
        CognitiveRef::CognitiveSchemaRevision(id) => {
            ("cognitive_schema_revision".into(), id.0.to_string())
        }
        CognitiveRef::CognitiveSeedVersion(id) => {
            ("cognitive_seed_version".into(), id.0.to_string())
        }
        CognitiveRef::Artifact(id) => ("artifact".into(), id.0.to_string()),
        CognitiveRef::SourceRegion(id) => ("source_region".into(), id.0.to_string()),
        CognitiveRef::DerivedRepresentation(id) => {
            ("derived_representation".into(), id.0.to_string())
        }
        CognitiveRef::DerivedRegion(id) => ("derived_region".into(), id.0.to_string()),
        CognitiveRef::Entity(id) => ("entity".into(), id.as_str().into()),
        CognitiveRef::Tag(id) => ("tag".into(), id.0.to_string()),
        CognitiveRef::Resource(id) => ("resource".into(), id.as_str().into()),
        CognitiveRef::ExternalObject(id) => ("external_object".into(), id.as_str().into()),
        CognitiveRef::Occurrence(id) => ("occurrence".into(), id.0.to_string()),
        CognitiveRef::Session(id) => ("session".into(), id.0.to_string()),
        CognitiveRef::Subject(id) => ("subject".into(), id.0.to_string()),
        CognitiveRef::WorkContext(id) => ("work_context".into(), id.0.to_string()),
        CognitiveRef::Association(id) => ("association".into(), id.0.to_string()),
    }
}

pub fn parse_reference(kind: &str, value: &str) -> Result<CognitiveRef> {
    let uuid = || {
        value.parse::<Uuid>().map_err(|error| {
            Error::new(
                ErrorOwner::Identity,
                ErrorCode::InvalidRequest,
                format!("invalid {kind} reference"),
            )
            .with_context("kind", kind)
            .with_source(error)
        })
    };
    Ok(match kind {
        "memory" => CognitiveRef::Memory(MemoryId(uuid()?)),
        "memory_revision" => CognitiveRef::MemoryRevision(MemoryRevisionId(uuid()?)),
        "episode" => CognitiveRef::Episode(EpisodeId(uuid()?)),
        "episode_revision" => CognitiveRef::EpisodeRevision(EpisodeRevisionId(uuid()?)),
        "journal" => CognitiveRef::Journal(JournalId(uuid()?)),
        "journal_revision" => CognitiveRef::JournalRevision(JournalRevisionId(uuid()?)),
        "cognitive_schema" => CognitiveRef::CognitiveSchema(CognitiveSchemaId(uuid()?)),
        "cognitive_schema_revision" => {
            CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(uuid()?))
        }
        "cognitive_seed_version" => {
            CognitiveRef::CognitiveSeedVersion(CognitiveSeedVersionId(uuid()?))
        }
        "artifact" => CognitiveRef::Artifact(ArtifactId(uuid()?)),
        "source_region" => CognitiveRef::SourceRegion(SourceRegionId(uuid()?)),
        "derived_representation" => {
            CognitiveRef::DerivedRepresentation(DerivedRepresentationId(uuid()?))
        }
        "derived_region" => CognitiveRef::DerivedRegion(DerivedRegionId(uuid()?)),
        "entity" => CognitiveRef::Entity(EntityRef::new(value)?),
        "tag" => CognitiveRef::Tag(TagId(uuid()?)),
        "resource" => CognitiveRef::Resource(ResourceRef::new(value)?),
        "external_object" => CognitiveRef::ExternalObject(ObjectRef::new(value)?),
        "occurrence" => CognitiveRef::Occurrence(OccurrenceId(uuid()?)),
        "session" => CognitiveRef::Session(SessionId(uuid()?)),
        "subject" => CognitiveRef::Subject(SubjectId(uuid()?)),
        "work_context" => CognitiveRef::WorkContext(WorkContextId(uuid()?)),
        "association" => CognitiveRef::Association(AssociationEvidenceId(uuid()?)),
        _ => {
            return Err(Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "unknown reference kind",
            ));
        }
    })
}
