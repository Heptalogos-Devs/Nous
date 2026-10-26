//! Pure content, identity, temporal and evidence rules.
use super::*;

impl CognitiveContent {
    pub fn validate(&self, evidence: &[EvidenceLink]) -> Result<()> {
        validate_links(evidence)?;
        match self {
            Self::MemorySemantics(value) => {
                validate_uuid(value.memory_id.as_str())?;
                positive_revision(value.memory_revision)?;
                value.epistemic.validate()?;
                validate_refs(&value.concepts, Some(CognitiveKind::Concept))?;
                for item in &value.qualifications {
                    text(item, "memory qualification", 65536)?;
                }
                match value.formation_mode {
                    FormationMode::Grounded => {
                        let anchor = value.grounding_observation.as_ref().ok_or_else(|| {
                            Error::invalid(
                                "grounded Memory semantics require an observation anchor",
                            )
                        })?;
                        validate_uuid(anchor.as_str())?;
                        if !evidence.iter().any(|link| {
                            link.polarity == EvidencePolarity::Support
                                && link.source
                                    == EvidenceSource::Observation {
                                        observation_id: anchor.clone(),
                                    }
                        }) {
                            return Err(Error::invalid(
                                "grounding observation must be explicit support",
                            ));
                        }
                    }
                    FormationMode::Synthesized => {
                        if value.grounding_observation.is_some() {
                            return Err(Error::invalid(
                                "synthesized Memory semantics have no single grounding observation",
                            ));
                        }
                        require_synthesis_support(evidence)?;
                    }
                }
            }
            Self::Concept(value) => {
                short(&value.label, "concept label")?;
                optional_text(&value.description, "concept description", 1_000_000)?;
                let mut aliases = BTreeSet::from([value.label.trim().to_lowercase()]);
                for alias in &value.aliases {
                    short(alias, "concept alias")?;
                    if !aliases.insert(alias.trim().to_lowercase()) {
                        return Err(Error::invalid("duplicate canonical concept label or alias"));
                    }
                }
                if let Some(canonical) = &value.canonical {
                    validate_ref_kind(canonical, CognitiveKind::Concept)?;
                }
                let mut members = BTreeSet::new();
                for member in &value.members {
                    member.validate()?;
                    if !members.insert(member) {
                        return Err(Error::invalid("Concept repeats a member"));
                    }
                }
                validate_lineage(&value.lineage, CognitiveKind::Concept)?;
            }
            Self::Entity(value) => value.validate()?,
            Self::Episode(value) => {
                text(&value.title, "episode title", 1024)?;
                short(&value.track, "episode track")?;
                value.experience.validate()?;
                text(&value.boundary_explanation, "episode boundary", 65536)?;
                if let Some(parent) = &value.parent {
                    validate_ref_kind(parent, CognitiveKind::Episode)?;
                }
                if value.members.is_empty() {
                    return Err(Error::invalid("episode requires ordered source members"));
                }
                let mut seen = BTreeSet::new();
                for (ordinal, member) in value.members.iter().enumerate() {
                    member.source.validate()?;
                    short(&member.role, "episode member role")?;
                    if member.ordinal as usize != ordinal || !seen.insert(member.source.clone()) {
                        return Err(Error::invalid(
                            "episode members require unique sources and contiguous zero-based ordinals",
                        ));
                    }
                }
                validate_lineage(&value.lineage, CognitiveKind::Episode)?;
            }
            Self::Journal(value) => {
                text(&value.title, "journal title", 1024)?;
                text(&value.narrative, "journal narrative", 1_000_000)?;
                value.period.validate()?;
                value.perspective.validate()?;
                if value.knowledge_cutoff < 0
                    || value.episodes.is_empty()
                    || value.points.is_empty()
                {
                    return Err(Error::invalid(
                        "journal requires a knowledge cutoff, source episodes and supported points",
                    ));
                }
                validate_refs(&value.episodes, Some(CognitiveKind::Episode))?;
                for point in &value.points {
                    text(&point.text, "journal point", 65536)?;
                    if point.evidence.is_empty() {
                        return Err(Error::invalid(
                            "every journal point needs explicit evidence",
                        ));
                    }
                    validate_links(&point.evidence)?;
                }
                validate_lineage(&value.lineage, CognitiveKind::Journal)?;
            }
            Self::Schema(value) => {
                text(&value.title, "schema title", 1024)?;
                text(&value.claim, "schema structural claim", 1_000_000)?;
                text(
                    &value.scope.description,
                    "schema applicability scope",
                    65536,
                )?;
                text(&value.boundary, "schema boundary", 65536)?;
                value.epistemic.validate()?;
                validate_refs(&value.scope.about, None)?;
                validate_refs(&value.scope.concepts, Some(CognitiveKind::Concept))?;
                if let Some(validity) = &value.scope.validity {
                    validity.validate()?;
                }
                if value.support.is_empty() {
                    return Err(Error::invalid(
                        "schema needs support; counterexamples and boundaries have independent roles",
                    ));
                }
                validate_sources(&value.support)?;
                validate_sources(&value.counterexamples)?;
                validate_sources(&value.boundary_cases)?;
                if value.formation_mode == FormationMode::Synthesized && value.support.len() < 2 {
                    return Err(Error::invalid(
                        "synthesized schema needs at least two distinct source inputs",
                    ));
                }
                validate_lineage(&value.lineage, CognitiveKind::Schema)?;
            }
            Self::Association(value) => {
                value.from.validate()?;
                value.to.validate()?;
                short(&value.relation, "association relation")?;
                optional_text(&value.description, "association description", 65536)?;
                if value.from.kind == value.to.kind && value.from.id == value.to.id {
                    return Err(Error::invalid(
                        "association endpoints must be different cognitive objects",
                    ));
                }
            }
        }
        let all = self
            .evidence_links()
            .into_iter()
            .chain(evidence.iter().cloned())
            .collect::<Vec<_>>();
        for link in &all {
            link.validate()?;
        }
        if !all
            .iter()
            .any(|link| link.polarity == EvidencePolarity::Support)
        {
            return Err(Error::invalid(
                "cognitive content needs positive source support",
            ));
        }
        Ok(())
    }
}

pub(super) fn validate_links(links: &[EvidenceLink]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for link in links {
        link.validate()?;
        if !seen.insert((link.source.clone(), link.polarity, link.relation)) {
            return Err(Error::invalid("duplicate evidence link"));
        }
    }
    Ok(())
}
pub(super) fn validate_sources(sources: &[EvidenceSource]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for source in sources {
        source.validate()?;
        if !seen.insert(source) {
            return Err(Error::invalid("duplicate source within evidence role"));
        }
    }
    Ok(())
}
pub(super) fn validate_refs(refs: &[CognitiveRef], kind: Option<CognitiveKind>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for reference in refs {
        reference.validate()?;
        if kind.is_some_and(|kind| reference.kind != kind) || !seen.insert(reference) {
            return Err(Error::invalid("reference kind or uniqueness is invalid"));
        }
    }
    Ok(())
}
pub(super) fn validate_ref_kind(reference: &CognitiveRef, kind: CognitiveKind) -> Result<()> {
    validate_refs(std::slice::from_ref(reference), Some(kind))
}
pub(super) fn validate_lineage(links: &[LineageLink], kind: CognitiveKind) -> Result<()> {
    let mut seen = BTreeSet::new();
    for link in links {
        link.target.validate()?;
        text(&link.reason, "lineage reason", 65536)?;
        if !seen.insert((link.target.clone(), link.relation as u8)) {
            return Err(Error::invalid("duplicate lineage link"));
        }
        let valid = match link.relation {
            LineageRelation::DerivedFrom => true,
            LineageRelation::MergedFrom | LineageRelation::SplitFrom => {
                matches!(kind, CognitiveKind::Concept | CognitiveKind::Schema)
                    && link.target.kind == kind
            }
            LineageRelation::ResegmentedFrom => {
                kind == CognitiveKind::Episode && link.target.kind == kind
            }
        };
        if !valid {
            return Err(Error::invalid(
                "lineage relation is incompatible with cognitive kinds",
            ));
        }
    }
    Ok(())
}
pub(super) fn reject_self_reference(
    body: &CognitiveContent,
    evidence: &[EvidenceLink],
    id: &CognitiveId,
) -> Result<()> {
    let references = body.references().into_iter().chain(
        evidence
            .iter()
            .filter_map(|link| link.source.cognitive_ref()),
    );
    if references
        .into_iter()
        .any(|reference| reference.kind == body.kind() && reference.id == id.as_str())
    {
        return Err(Error::invalid(
            "a cognitive revision cannot depend on any revision of its own object",
        ));
    }
    Ok(())
}
pub(super) fn require_synthesis_support(evidence: &[EvidenceLink]) -> Result<()> {
    let inputs = evidence
        .iter()
        .filter(|link| link.polarity == EvidencePolarity::Support)
        .map(|link| &link.source)
        .collect::<BTreeSet<_>>();
    if inputs.len() < 2 {
        return Err(Error::invalid(
            "synthesized cognition needs at least two distinct source inputs",
        ));
    }
    Ok(())
}
pub(super) fn positive_revision(revision: i64) -> Result<()> {
    if revision <= 0 {
        return Err(Error::invalid("exact revision must be positive"));
    }
    Ok(())
}
pub(super) fn short(value: &str, field: &str) -> Result<()> {
    crate::validate_name(value, field)
}
pub(super) fn text(value: &str, field: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!("{field} is required")));
    }
    optional_text(value, field, max)
}
pub(super) fn optional_text(value: &str, field: &str, max: usize) -> Result<()> {
    if value.len() > max || value.contains('\0') {
        return Err(Error::invalid(format!(
            "{field} exceeds its bound or contains NUL"
        )));
    }
    Ok(())
}
