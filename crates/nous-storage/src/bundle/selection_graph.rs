use nous_domain::{media::*, *};
use std::collections::BTreeSet;

/// Stable identities select retained history, rather than only the latest revision.
#[derive(Default)]
pub(super) struct Selection(pub BTreeSet<(String, String)>);
impl Selection {
    pub fn add(&mut self, kind: &str, id: &str) -> bool {
        self.0.insert((kind.into(), id.into()))
    }
    pub fn has(&self, kind: &str, id: &str) -> bool {
        self.0.contains(&(kind.into(), id.into()))
    }
    pub fn target(&mut self, value: &RetrievalTarget) {
        match value {
            RetrievalTarget::Cognition(v) => {
                self.add(cognitive_kind(v.kind), &v.id);
            }
            RetrievalTarget::Media(v) => self.media(v),
        }
    }
    pub fn contains_target(&self, value: &RetrievalTarget) -> bool {
        match value {
            RetrievalTarget::Cognition(v) => self.has(cognitive_kind(v.kind), &v.id),
            RetrievalTarget::Media(v) => self.contains_media(v),
        }
    }
    pub fn media(&mut self, value: &MediaReference) {
        let (kind, id) = media_key(value);
        self.add(kind, id);
    }
    pub fn contains_media(&self, value: &MediaReference) -> bool {
        let (kind, id) = media_key(value);
        self.has(kind, id)
    }
    pub fn source(&mut self, value: &EvidenceSource) {
        if let EvidenceSource::Observation { observation_id } = value {
            self.add("observation", observation_id.as_str());
        } else if let Some(target) = value.retrieval_target() {
            self.target(&target);
        }
    }
    pub fn contains_source(&self, value: &EvidenceSource) -> bool {
        match value {
            EvidenceSource::Observation { observation_id } => {
                self.has("observation", observation_id.as_str())
            }
            _ => value
                .retrieval_target()
                .is_some_and(|v| self.contains_target(&v)),
        }
    }
    pub fn read_source(&mut self, value: &SourceReadRef) -> Result<()> {
        match value {
            SourceReadRef::Observation { id } => {
                self.add("observation", id.as_str());
            }
            SourceReadRef::Media {
                reference,
                observation_id,
            } => {
                self.media(reference);
                if let Some(id) = observation_id {
                    self.add("observation", id.as_str());
                }
            }
            SourceReadRef::Cognition { reference } => {
                self.target(&RetrievalTarget::Cognition(reference.clone()))
            }
            SourceReadRef::WorkContext { .. } => {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "bundle producer references WorkContext, which is not represented in SubjectPackage",
                ));
            }
        }
        Ok(())
    }
    pub fn all(&mut self, p: &SubjectPackage) {
        for v in &p.materials {
            if v.details.source_type.as_deref() == Some("nous_bundle")
                && !p
                    .observations
                    .iter()
                    .any(|event| event.material_id == v.material.id)
            {
                continue;
            }
            self.add("material", v.material.id.as_str());
        }
        for v in &p.observations {
            self.add("observation", v.id.as_str());
        }
        for v in &p.memories {
            self.add("memory", v.id.as_str());
        }
        for v in &p.cognition {
            self.add("cognition", v.id.as_str());
        }
        for v in &p.source_regions {
            self.add("source_region", v.id.as_str());
        }
        for v in &p.representations {
            self.add("representation", v.id.as_str());
        }
        for v in &p.derived_regions {
            self.add("derived_region", v.id.as_str());
        }
        for v in &p.purges {
            self.add("memory", v.id.as_str());
        }
    }
    /// Linked entities and relations bring their actual endpoints. Aggregates
    /// follow A-Memorix's closed-episode rule and require every retained member.
    pub fn related(&mut self, p: &SubjectPackage) {
        for v in &p.source_regions {
            if self.has("material", v.material_id.as_str()) {
                self.add("source_region", v.id.as_str());
            }
        }
        for v in &p.representations {
            let inputs = v.source_references();
            if !inputs.is_empty()
                && inputs.iter().all(|s| self.contains_media(s))
                && v.observation_id
                    .as_ref()
                    .is_none_or(|id| self.has("observation", id.as_str()))
            {
                self.add("representation", v.id.as_str());
            }
        }
        for v in &p.derived_regions {
            if self.has("representation", v.representation.id.as_str()) {
                self.add("derived_region", v.id.as_str());
            }
        }
        for v in &p.memories {
            if self.has("observation", v.observation_id.as_str()) {
                self.add("memory", v.id.as_str());
            }
        }
        for v in &p.tombstones {
            if v.sources.iter().any(|source| self.contains_source(source)) {
                self.target(&v.target);
            }
        }
        for v in &p.cognition {
            let sources = v.sources();
            let linked = if matches!(
                v.body,
                CognitiveContent::Episode(_) | CognitiveContent::Journal(_)
            ) {
                sources.iter().all(|s| self.contains_source(s))
            } else {
                sources.iter().any(|s| self.contains_source(s))
            };
            if !sources.is_empty() && linked {
                self.add("cognition", v.id.as_str());
            }
        }
    }
    pub fn dependencies(&mut self, p: &SubjectPackage) -> Result<()> {
        for v in &p.exposures {
            if self.has("observation", v.observation_id.as_str()) {
                self.media(&v.locator);
            }
        }
        for v in &p.observations {
            if self.has("observation", v.id.as_str()) {
                self.add("material", v.material_id.as_str());
            }
        }
        for v in &p.memories {
            if self.has("memory", v.id.as_str()) {
                self.add("observation", v.observation_id.as_str());
                self.add("material", v.material_id.as_str());
                self.media(&v.source);
                if let Some(id) = &v.model_call_id {
                    self.add("model_call", id.as_str());
                }
            }
        }
        for v in &p.cognition {
            if self.has("cognition", v.id.as_str()) {
                for source in v.sources() {
                    self.source(&source);
                }
                if let Some(id) = &v.producer_call_id {
                    self.add("model_call", id.as_str());
                }
            }
        }
        for v in &p.source_regions {
            if self.has("source_region", v.id.as_str()) {
                for source in v.source_references() {
                    self.media(&source);
                }
            }
        }
        for v in &p.representations {
            if self.has("representation", v.id.as_str()) {
                for source in v.source_references() {
                    self.media(&source);
                }
                if let Some(previous) = &v.previous {
                    self.add("representation", previous.id.as_str());
                }
                if let Some(id) = &v.observation_id {
                    self.add("observation", id.as_str());
                }
                if let Some(id) = &v.producer.model_call_id {
                    self.add("model_call", id.as_str());
                }
            }
        }
        for v in &p.derived_regions {
            if self.has("derived_region", v.id.as_str()) {
                for source in v.source_references() {
                    self.media(&source);
                }
            }
        }
        for v in &p.tombstones {
            if self.contains_target(&v.target) {
                for source in &v.sources {
                    self.source(source);
                }
            }
        }
        for v in &p.vectors.associations {
            if v.record
                .target
                .item()
                .is_some_and(|t| self.contains_target(t))
            {
                self.add("model_call", v.record.model_call_id.as_str());
                for source in v.input.source_requirements() {
                    self.read_source(&source)?;
                }
            }
        }
        Ok(())
    }
    pub fn retain(&self, p: &mut SubjectPackage, all: bool) {
        p.materials
            .retain(|v| self.has("material", v.material.id.as_str()));
        p.observations
            .retain(|v| self.has("observation", v.id.as_str()));
        for original in &mut p.materials {
            if let Some(records) = original
                .details
                .metadata
                .get_mut("image_occurrences")
                .and_then(serde_json::Value::as_array_mut)
            {
                records.retain(|record| {
                    record
                        .get("observation_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| self.has("observation", id))
                });
            }
        }
        p.exposures.retain(|v| {
            self.has("observation", v.observation_id.as_str()) && self.contains_media(&v.locator)
        });
        p.memories.retain(|v| self.has("memory", v.id.as_str()));
        p.cognition.retain(|v| self.has("cognition", v.id.as_str()));
        p.source_regions
            .retain(|v| self.has("source_region", v.id.as_str()));
        p.representations
            .retain(|v| self.has("representation", v.id.as_str()));
        p.derived_regions
            .retain(|v| self.has("derived_region", v.id.as_str()));
        p.purges.retain(|v| self.has("memory", v.id.as_str()));
        p.tombstones.retain(|v| self.contains_target(&v.target));
        p.vectors.associations.retain(|v| {
            all || v
                .record
                .target
                .item()
                .is_some_and(|t| self.contains_target(t))
        });
        let content: BTreeSet<_> = p
            .vectors
            .associations
            .iter()
            .map(|v| (v.record.space_id.clone(), v.record.source_hash.clone()))
            .collect();
        p.vectors
            .contents
            .retain(|v| all || content.contains(&(v.space_id.clone(), v.source_hash.clone())));
        let spaces: BTreeSet<_> = p.vectors.contents.iter().map(|v| &v.space_id).collect();
        p.vectors.spaces.retain(|v| spaces.contains(&v.id));
    }
}
fn cognitive_kind(kind: CognitiveKind) -> &'static str {
    if kind == CognitiveKind::Memory {
        "memory"
    } else {
        "cognition"
    }
}
fn media_key(v: &MediaReference) -> (&'static str, &str) {
    match v {
        MediaReference::Original { material_id } => ("material", material_id.as_str()),
        MediaReference::SourceRegion { id } => ("source_region", id.as_str()),
        MediaReference::Representation { reference } => ("representation", reference.id.as_str()),
        MediaReference::DerivedRegion { id } => ("derived_region", id.as_str()),
    }
}
