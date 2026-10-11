// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[expect(
    clippy::type_complexity,
    reason = "the tuple mirrors the normalized support columns written atomically"
)]
pub(super) fn association_basis_parts(
    basis: &AssociationBasis,
) -> Result<(
    String,
    String,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
)> {
    match basis {
        AssociationBasis::UseEvent(value) => Ok((
            "use_event".into(),
            value.canonical_key(),
            BasisRole::Contextual.as_str().into(),
            None,
            None,
            None,
            None,
        )),
        AssociationBasis::Revision(RevisionBasis::Evidence(value)) => {
            let (source_region, derived_representation, derived_region) = match value.locator {
                EvidenceLocator::WholeOccurrence => (None, None, None),
                EvidenceLocator::SourceRegion(id) => (Some(id.0), None, None),
                EvidenceLocator::DerivedRepresentation(id) => (None, Some(id.0), None),
                EvidenceLocator::DerivedRegion(id) => (None, None, Some(id.0)),
            };
            Ok((
                "evidence".into(),
                value.canonical_key(),
                value.basis_role.as_str().into(),
                Some(value.occurrence_id.0),
                source_region,
                derived_representation,
                derived_region,
            ))
        }
        AssociationBasis::Revision(RevisionBasis::CognitionDependency(value)) => {
            let (kind, reference) = reference_parts(&value.target_revision);
            Ok((
                kind,
                reference,
                value.basis_role.as_str().into(),
                None,
                None,
                None,
                None,
            ))
        }
        AssociationBasis::Revision(RevisionBasis::Seed(_)) => Err(Error::new(
            ErrorOwner::Concept,
            ErrorCode::InvalidRequest,
            "Association support cannot use Cognitive Seed support",
        )),
    }
}

pub(super) fn validate_association_endpoint(reference: &CognitiveRef) -> Result<()> {
    match reference {
        CognitiveRef::Memory(_) | CognitiveRef::CognitiveSchema(_) => Err(Error::new(
            ErrorOwner::Concept,
            ErrorCode::InvalidRequest,
            "association cognition endpoints must be exact revisions",
        )),
        CognitiveRef::MemoryRevision(_)
        | CognitiveRef::CognitiveSchemaRevision(_)
        | CognitiveRef::EpisodeRevision(_)
        | CognitiveRef::JournalRevision(_)
        | CognitiveRef::Entity(_)
        | CognitiveRef::Tag(_)
        | CognitiveRef::Resource(_) => Ok(()),
        _ => Err(Error::new(
            ErrorOwner::Concept,
            ErrorCode::InvalidRequest,
            "association endpoint must be cognition revision, Entity, Tag, or Resource",
        )),
    }
}

pub(super) fn parse_use_event_key(value: &str) -> Result<(String, Uuid)> {
    let (_, rest) = value.split_once(':').ok_or_else(|| {
        Error::new(
            ErrorOwner::Concept,
            ErrorCode::InternalFailure,
            "invalid association UseEvent key",
        )
    })?;
    let (consumer, event) = rest.rsplit_once(':').ok_or_else(|| {
        Error::new(
            ErrorOwner::Concept,
            ErrorCode::InternalFailure,
            "invalid association UseEvent key",
        )
    })?;
    let event = event.parse().map_err(|error| {
        Error::new(
            ErrorOwner::Concept,
            ErrorCode::InternalFailure,
            "invalid association UseEvent id",
        )
        .with_source(error)
    })?;
    Ok((consumer.into(), event))
}

pub(super) fn validate_association_input(input: &CreateAssociationRequest) -> Result<()> {
    if input.relation_kind.is_empty()
        || input.relation_kind.len() > 128
        || !input
            .relation_kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.:-".contains(&b))
    {
        return Err(Error::new(
            ErrorOwner::Concept,
            ErrorCode::InvalidRequest,
            "relation_kind is invalid",
        ));
    }
    if !(1..=16).contains(&input.basis.len()) {
        return Err(Error::new(
            ErrorOwner::Concept,
            ErrorCode::InvalidRequest,
            "association needs 1..16 basis",
        ));
    }
    input.valid_time.validate()?;
    validate_association_endpoint(&input.from)?;
    validate_association_endpoint(&input.to)?;
    if input.from == input.to {
        return Err(Error::new(
            ErrorOwner::Concept,
            ErrorCode::InvalidRequest,
            "association endpoints must differ",
        ));
    }
    if input.relation_kind == "tag_attachment" {
        let cognition = |reference: &CognitiveRef| {
            matches!(
                reference,
                CognitiveRef::MemoryRevision(_)
                    | CognitiveRef::CognitiveSchemaRevision(_)
                    | CognitiveRef::EpisodeRevision(_)
                    | CognitiveRef::JournalRevision(_)
            )
        };
        if input.polarity != AssociationPolarity::Positive
            || !(cognition(&input.from) && matches!(input.to, CognitiveRef::Tag(_)))
        {
            return Err(Error::new(
                ErrorOwner::Concept,
                ErrorCode::InvalidRequest,
                "tag_attachment requires positive cognition to Tag endpoints",
            ));
        }
    }
    Ok(())
}
