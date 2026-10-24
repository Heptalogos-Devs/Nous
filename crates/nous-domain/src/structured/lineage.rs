//! Atomic concept and episode reorganization requests.
use super::*;

/// Scoped exact heads for atomic semantic maintenance operations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredExpectation {
    pub id: CognitiveId,
    pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeConceptsRequest {
    pub survivor: StructuredExpectation,
    pub retired: Vec<StructuredExpectation>,
    pub evidence: Vec<EvidenceLink>,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitConceptRequest {
    pub parent: StructuredExpectation,
    pub children: Vec<ConceptContent>,
    pub evidence: Vec<EvidenceLink>,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResegmentEpisodesRequest {
    pub sources: Vec<StructuredExpectation>,
    pub segments: Vec<EpisodeContent>,
    pub evidence: Vec<EvidenceLink>,
    pub reason: String,
}
impl StructuredExpectation {
    pub fn validate(&self) -> Result<()> {
        validate_uuid(self.id.as_str())?;
        positive_revision(self.expected_revision)
    }
}
impl MergeConceptsRequest {
    pub fn validate(&self) -> Result<()> {
        self.survivor.validate()?;
        text(&self.reason, "merge reason", 65536)?;
        validate_lineage_basis(&self.evidence)?;
        if !(1..=7).contains(&self.retired.len()) {
            return Err(Error::invalid("merge requires 1..7 retired concepts"));
        }
        let mut ids = BTreeSet::from([self.survivor.id.clone()]);
        for target in &self.retired {
            target.validate()?;
            if !ids.insert(target.id.clone()) {
                return Err(Error::invalid("duplicate merge concept"));
            }
        }
        Ok(())
    }
}
impl SplitConceptRequest {
    pub fn validate(&self) -> Result<()> {
        self.parent.validate()?;
        text(&self.reason, "split reason", 65536)?;
        validate_lineage_basis(&self.evidence)?;
        if !(2..=8).contains(&self.children.len()) {
            return Err(Error::invalid("split requires 2..8 child concepts"));
        }
        let mut aliases = BTreeSet::new();
        for child in &self.children {
            CognitiveContent::Concept(child.clone()).validate(&self.evidence)?;
            if child.canonical.is_some() {
                return Err(Error::invalid(
                    "split children must have their own canonical identity",
                ));
            }
            for key in child.alias_keys() {
                if !aliases.insert(key) {
                    return Err(Error::invalid("split children share a label or alias"));
                }
            }
        }
        Ok(())
    }
}
impl ResegmentEpisodesRequest {
    pub fn validate(&self) -> Result<()> {
        text(&self.reason, "resegmentation reason", 65536)?;
        validate_links(&self.evidence)?;
        if !(1..=8).contains(&self.sources.len()) || !(1..=16).contains(&self.segments.len()) {
            return Err(Error::invalid(
                "resegmentation requires 1..8 source episodes and 1..16 segments",
            ));
        }
        let mut ids = BTreeSet::new();
        for target in &self.sources {
            target.validate()?;
            if !ids.insert(target.id.clone()) {
                return Err(Error::invalid("duplicate resegmentation source"));
            }
        }
        let mut sources = BTreeSet::new();
        let mut count = 0;
        for segment in &self.segments {
            CognitiveContent::Episode(segment.clone()).validate(&self.evidence)?;
            for member in &segment.members {
                count += 1;
                if !sources.insert(member.source.clone()) {
                    return Err(Error::invalid("resegmentation repeats a source member"));
                }
            }
        }
        if count > 2048 {
            return Err(Error::invalid(
                "resegmentation exceeds 2048 ordered members",
            ));
        }
        Ok(())
    }
}
fn validate_lineage_basis(evidence: &[EvidenceLink]) -> Result<()> {
    if !(1..=16).contains(&evidence.len()) {
        return Err(Error::invalid(
            "concept lineage requires 1..16 exact source links",
        ));
    }
    validate_links(evidence)
}
