//! Ordered derivation graph, producer identity and three interpretation strategies.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DerivationStrategy {
    DescriptionOnly,
    DirectStructured,
    DescribeThenStructure,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationKind {
    ExtractedText,
    Ocr,
    Transcript,
    AudioDescription,
    ImageDescription,
    SceneDescription,
    Summary,
    StructuredInterpretation,
    ResourceSynopsis,
    SampledFrames,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DerivationInputRole {
    Source,
    Context,
    Description,
    Transcript,
    ExtractedText,
    SampledFrame,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivationInput {
    pub ordinal: u32,
    pub source: MediaReference,
    pub role: DerivationInputRole,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelMode {
    Original,
    Representation,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelAccess {
    pub visual: ChannelMode,
    pub audio: ChannelMode,
    pub source_text: ChannelMode,
}
impl ChannelAccess {
    pub fn represented(self) -> Self {
        let downgrade = |mode| {
            if mode == ChannelMode::Unavailable {
                mode
            } else {
                ChannelMode::Representation
            }
        };
        Self {
            visual: downgrade(self.visual),
            audio: downgrade(self.audio),
            source_text: downgrade(self.source_text),
        }
    }
    pub fn mode(self, channel: EvidenceChannel) -> ChannelMode {
        match channel {
            EvidenceChannel::Visual => self.visual,
            EvidenceChannel::Audio => self.audio,
            EvidenceChannel::SourceText => self.source_text,
        }
    }
    fn has_original(self) -> bool {
        [self.visual, self.audio, self.source_text].contains(&ChannelMode::Original)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivationProducer {
    pub signature: String,
    pub implementation: String,
    pub model_call_id: Option<ModelCallId>,
    pub prompt_digest: Option<String>,
    pub schema_digest: Option<String>,
    pub config_digest: String,
    pub preprocessing_digest: Option<String>,
}
impl DerivationProducer {
    pub fn validate(&self) -> Result<()> {
        valid_digest(&self.signature)?;
        valid_digest(&self.config_digest)?;
        bounded_text(&self.implementation, 256, "producer implementation", true)?;
        if let Some(id) = &self.model_call_id {
            validate_uuid(id.as_str())?;
        }
        if let Some(hash) = &self.prompt_digest {
            valid_digest(hash)?;
        }
        if let Some(hash) = &self.schema_digest {
            valid_digest(hash)?;
        }
        if let Some(hash) = &self.preprocessing_digest {
            valid_digest(hash)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlobReference {
    pub content_hash: String,
    pub media_type: String,
    pub byte_len: u64,
}
impl BlobReference {
    pub fn validate(&self) -> Result<()> {
        valid_digest(&self.content_hash)?;
        canonical_media_type(&self.media_type)?;
        if self.byte_len == 0 || self.byte_len > i64::MAX as u64 {
            return Err(Error::invalid("derived blob length is invalid"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepresentationPayload {
    Text {
        text: String,
    },
    Interpretation {
        value: MediaInterpretation,
    },
    Artifact {
        blob: BlobReference,
        metadata: MediaMetadata,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivedRepresentation {
    pub id: DerivedRepresentationId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub revision: i64,
    pub inputs: Vec<DerivationInput>,
    pub strategy: DerivationStrategy,
    pub kind: RepresentationKind,
    pub payload: RepresentationPayload,
    pub producer: DerivationProducer,
    pub access: ChannelAccess,
    pub observation_id: Option<ObservationId>,
    pub source_event_kind: Option<crate::ObservationKind>,
    /// Media timestamps are relative to this exact supplied locator, independent of UTC.
    pub timeline: Option<MediaReference>,
    pub formed_at: TimePoint,
    pub recorded_at: String,
    pub commit_seq: i64,
    pub previous: Option<RepresentationRef>,
    #[serde(default)]
    pub quality: DerivationQuality,
}
impl DerivedRepresentation {
    pub fn reference(&self) -> RepresentationRef {
        RepresentationRef {
            id: self.id.clone(),
            revision: self.revision,
        }
    }
    pub fn source_references(&self) -> Vec<MediaReference> {
        let mut references = self
            .inputs
            .iter()
            .map(|input| input.source.clone())
            .collect::<Vec<_>>();
        if let Some(timeline) = &self.timeline
            && !references.contains(timeline)
        {
            references.push(timeline.clone());
        }
        references
    }
    /// Stable replay material excludes result bytes, model attempt IDs and clocks.
    /// Storage hashes this after binding inputs/producer to the admitted snapshot.
    pub fn derivation_key_material(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&(
            &self.namespace_id,
            &self.subject_id,
            &self.inputs,
            self.strategy,
            self.kind,
            &self.producer.signature,
            &self.producer.implementation,
            &self.producer.prompt_digest,
            &self.producer.schema_digest,
            &self.producer.config_digest,
            &self.producer.preprocessing_digest,
            &self.previous,
            &self.timeline,
            &self.observation_id,
            &self.source_event_kind,
        ))?)
    }
    pub fn validate(&self, duration_ms: Option<u64>) -> Result<()> {
        self.id.validate()?;
        validate_uuid(self.namespace_id.as_str())?;
        validate_uuid(self.subject_id.as_str())?;
        self.producer.validate()?;
        self.formed_at.validate()?;
        bounded_text(&self.recorded_at, 128, "recorded_at", true)?;
        if self.commit_seq < 0 || self.inputs.is_empty() || self.inputs.len() > 64 {
            return Err(Error::invalid(
                "derivation needs 1..64 ordered inputs and a nonnegative commit",
            ));
        }
        let mut seen = BTreeSet::new();
        for (ordinal, input) in self.inputs.iter().enumerate() {
            input.source.validate()?;
            if matches!(
                input.role,
                DerivationInputRole::Description
                    | DerivationInputRole::Transcript
                    | DerivationInputRole::ExtractedText
            ) && !matches!(
                input.source,
                MediaReference::Representation { .. } | MediaReference::DerivedRegion { .. }
            ) {
                return Err(Error::invalid(
                    "textual derivation input must name a committed representation or region",
                ));
            }
            if input.ordinal as usize != ordinal
                || !seen.insert(input.source.clone())
                || matches!(&input.source,MediaReference::Representation{reference}if reference.id==self.id)
            {
                return Err(Error::invalid(
                    "derivation inputs are unordered, repeated or self-referential",
                ));
            }
        }
        if let Some(previous) = &self.previous {
            previous.id.validate()?;
            if previous.id != self.id || previous.revision.checked_add(1) != Some(self.revision) {
                return Err(Error::invalid(
                    "revision history requires the same representation identity and next revision",
                ));
            }
        } else if self.revision != 1 {
            return Err(Error::invalid(
                "a new derivation chain starts at revision 1",
            ));
        }
        if let Some(observation) = &self.observation_id {
            validate_uuid(observation.as_str())?;
        }
        if self.observation_id.is_some() != self.source_event_kind.is_some() {
            return Err(Error::invalid(
                "derivation source event id and kind must be supplied together",
            ));
        }
        if let Some(timeline) = &self.timeline {
            timeline.validate()?;
        }
        self.quality.validate(duration_ms)?;
        match (&self.kind, &self.payload) {
            (
                RepresentationKind::StructuredInterpretation,
                RepresentationPayload::Interpretation { value },
            ) => {
                if self.producer.schema_digest.is_none() {
                    return Err(Error::invalid(
                        "structured interpretation requires its producer schema identity",
                    ));
                }
                if self.strategy == DerivationStrategy::DescriptionOnly {
                    return Err(Error::invalid(
                        "description_only does not produce structured interpretation",
                    ));
                }
                if self.strategy == DerivationStrategy::DirectStructured
                    && (!self.access.has_original()
                        || !self.inputs.iter().any(|input| {
                            matches!(
                                input.role,
                                DerivationInputRole::Source
                                    | DerivationInputRole::SampledFrame
                                    | DerivationInputRole::ExtractedText
                            )
                        }))
                {
                    return Err(Error::invalid(
                        "direct_structured requires actual original input",
                    ));
                }
                if self.strategy == DerivationStrategy::DescribeThenStructure
                    && (!self.access.has_original()
                        || !self.inputs.iter().any(|input| {
                            matches!(
                                input.role,
                                DerivationInputRole::Description | DerivationInputRole::Transcript
                            )
                        })
                        || !self.inputs.iter().any(|input| {
                            matches!(
                                input.role,
                                DerivationInputRole::Source | DerivationInputRole::SampledFrame
                            )
                        }))
                {
                    return Err(Error::invalid(
                        "second-stage media structuring requires both exact media and committed description inputs",
                    ));
                }
                value.validate(self.access, &seen, duration_ms)?;
            }
            (RepresentationKind::StructuredInterpretation, _)
            | (_, RepresentationPayload::Interpretation { .. }) => {
                return Err(Error::invalid(
                    "interpretation payload and representation kind disagree",
                ));
            }
            (_, RepresentationPayload::Text { text }) => {
                bounded_text(text, 1_000_000, "representation text", true)?
            }
            (_, RepresentationPayload::Artifact { blob, metadata }) => {
                blob.validate()?;
                metadata.validate(&blob.media_type)?;
            }
        }
        Ok(())
    }
    /// The invocation/worker supplies actual access from the saved request, never
    /// from a model's claim about which source channels it received.
    pub fn validate_actual_access(
        &self,
        actual: ChannelAccess,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        if self.access != actual {
            return Err(Error::invalid(
                "derived input access differs from the actual invocation",
            ));
        }
        self.validate(duration_ms)
    }
}
