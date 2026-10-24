//! Cognitive revision envelope and exact-write requests.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredObject {
    pub id: CognitiveId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub revision: i64,
    pub body: CognitiveContent,
    pub evidence: Vec<EvidenceLink>,
    pub times: CognitiveTimes,
    pub producer_call_id: Option<ModelCallId>,
    pub recorded_at: String,
    pub commit_seq: i64,
    pub deleted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateStructuredRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub expected_revision: i64,
    pub body: CognitiveContent,
    pub evidence: Vec<EvidenceLink>,
    pub times: CognitiveTimes,
    pub producer_call_id: Option<ModelCallId>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviseStructuredRequest {
    pub id: CognitiveId,
    pub expected_revision: i64,
    pub body: CognitiveContent,
    pub evidence: Vec<EvidenceLink>,
    pub times: CognitiveTimes,
    pub producer_call_id: Option<ModelCallId>,
}

impl StructuredObject {
    pub fn is_active(&self) -> bool {
        !self.deleted
            && match &self.body {
                CognitiveContent::Concept(concept) => concept.is_active(),
                _ => true,
            }
    }
    pub fn kind(&self) -> CognitiveKind {
        self.body.kind()
    }
    pub fn reference(&self) -> CognitiveRef {
        CognitiveRef::structured(self.kind(), &self.id, self.revision)
    }
    pub fn evidence_links(&self) -> Vec<EvidenceLink> {
        dedup_links(
            self.evidence
                .iter()
                .cloned()
                .chain(self.body.evidence_links())
                .collect(),
        )
    }
    pub fn sources(&self) -> Vec<EvidenceSource> {
        self.evidence_links()
            .into_iter()
            .map(|link| link.source)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn references(&self) -> Vec<CognitiveRef> {
        self.sources()
            .iter()
            .filter_map(EvidenceSource::cognitive_ref)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn validate(&self) -> Result<()> {
        validate_uuid(self.id.as_str())?;
        validate_uuid(self.namespace_id.as_str())?;
        validate_uuid(self.subject_id.as_str())?;
        positive_revision(self.revision)?;
        self.body.validate(&self.evidence)?;
        self.times.validate()?;
        text(&self.recorded_at, "recorded_at", 128)?;
        if self.commit_seq < 0 {
            return Err(Error::invalid("commit_seq must be nonnegative"));
        }
        if let Some(id) = &self.producer_call_id {
            validate_uuid(id.as_str())?;
        }
        reject_self_reference(&self.body, &self.evidence, &self.id)?;
        Ok(())
    }
}
impl CreateStructuredRequest {
    pub fn validate(&self) -> Result<()> {
        if self.expected_revision != 0 {
            return Err(Error::invalid("create expected_revision must be zero"));
        }
        validate_uuid(self.namespace_id.as_str())?;
        validate_uuid(self.subject_id.as_str())?;
        self.body.validate(&self.evidence)?;
        self.times.validate()?;
        if let Some(id) = &self.producer_call_id {
            validate_uuid(id.as_str())?;
        }
        Ok(())
    }
}
impl ReviseStructuredRequest {
    pub fn validate(&self) -> Result<()> {
        positive_revision(self.expected_revision)?;
        validate_uuid(self.id.as_str())?;
        self.body.validate(&self.evidence)?;
        self.times.validate()?;
        reject_self_reference(&self.body, &self.evidence, &self.id)?;
        if let Some(id) = &self.producer_call_id {
            validate_uuid(id.as_str())?;
        }
        Ok(())
    }
}
