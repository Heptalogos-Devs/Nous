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
    Ok(match kind {
        "memory" => CognitiveRef::Memory(MemoryId(value.parse().map_err(|error| {
            Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "invalid memory ref",
            )
            .with_source(error)
        })?)),
        "memory_revision" => {
            CognitiveRef::MemoryRevision(MemoryRevisionId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid memory revision ref",
                )
                .with_source(error)
            })?))
        }
        "episode" => CognitiveRef::Episode(EpisodeId(value.parse().map_err(|error| {
            Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "invalid episode ref",
            )
            .with_source(error)
        })?)),
        "episode_revision" => {
            CognitiveRef::EpisodeRevision(EpisodeRevisionId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid episode revision ref",
                )
                .with_source(error)
            })?))
        }
        "journal" => CognitiveRef::Journal(JournalId(value.parse().map_err(|error| {
            Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "invalid Journal ref",
            )
            .with_source(error)
        })?)),
        "journal_revision" => {
            CognitiveRef::JournalRevision(JournalRevisionId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid Journal revision ref",
                )
                .with_source(error)
            })?))
        }
        "cognitive_schema" => {
            CognitiveRef::CognitiveSchema(CognitiveSchemaId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid CognitiveSchema ref",
                )
                .with_source(error)
            })?))
        }
        "cognitive_schema_revision" => CognitiveRef::CognitiveSchemaRevision(
            CognitiveSchemaRevisionId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid CognitiveSchema revision ref",
                )
                .with_source(error)
            })?),
        ),
        "cognitive_seed_version" => CognitiveRef::CognitiveSeedVersion(CognitiveSeedVersionId(
            value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid Cognitive Seed version ref",
                )
                .with_source(error)
            })?,
        )),
        "artifact" => CognitiveRef::Artifact(ArtifactId(value.parse().map_err(|error| {
            Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "invalid artifact ref",
            )
            .with_source(error)
        })?)),
        "source_region" => {
            CognitiveRef::SourceRegion(SourceRegionId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid source region ref",
                )
                .with_source(error)
            })?))
        }
        "derived_representation" => CognitiveRef::DerivedRepresentation(DerivedRepresentationId(
            value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid derived ref",
                )
                .with_source(error)
            })?,
        )),
        "derived_region" => {
            CognitiveRef::DerivedRegion(DerivedRegionId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid derived region ref",
                )
                .with_source(error)
            })?))
        }
        "entity" => CognitiveRef::Entity(EntityRef::new(value)?),
        "tag" => CognitiveRef::Tag(TagId(value.parse().map_err(|error| {
            Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "invalid tag ref",
            )
            .with_source(error)
        })?)),
        "resource" => CognitiveRef::Resource(ResourceRef::new(value)?),
        "external_object" => CognitiveRef::ExternalObject(ObjectRef::new(value)?),
        "occurrence" => CognitiveRef::Occurrence(OccurrenceId(value.parse().map_err(|error| {
            Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "invalid occurrence ref",
            )
            .with_source(error)
        })?)),
        "session" => CognitiveRef::Session(SessionId(value.parse().map_err(|error| {
            Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "invalid session ref",
            )
            .with_source(error)
        })?)),
        "subject" => CognitiveRef::Subject(SubjectId(value.parse().map_err(|error| {
            Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "invalid Subject ref",
            )
            .with_source(error)
        })?)),
        "work_context" => {
            CognitiveRef::WorkContext(WorkContextId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid WorkContext ref",
                )
                .with_source(error)
            })?))
        }
        "association" => {
            CognitiveRef::Association(AssociationEvidenceId(value.parse().map_err(|error| {
                Error::new(
                    crate::ErrorOwner::Identity,
                    crate::ErrorCode::InvalidRequest,
                    "invalid Association ref",
                )
                .with_source(error)
            })?))
        }
        _ => {
            return Err(Error::new(
                crate::ErrorOwner::Identity,
                crate::ErrorCode::InvalidRequest,
                "unknown reference kind",
            ));
        }
    })
}
