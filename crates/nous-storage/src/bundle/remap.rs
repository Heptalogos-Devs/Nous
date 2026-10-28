use nous_domain::{media::*, *};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) struct Remap {
    pub ids: BTreeMap<String, String>,
    namespace: NamespaceId,
    subject: SubjectId,
    sequence: i64,
    recorded_at: String,
    placement: BundlePlacement,
}
impl Remap {
    pub fn new(
        package: &SubjectPackage,
        namespace: &NamespaceId,
        subject: &SubjectId,
        sequence: i64,
        recorded_at: String,
        placement: &BundlePlacement,
    ) -> Self {
        let mut ids = BTreeMap::new();
        ids.insert(
            package.subject.namespace_id.to_string(),
            namespace.to_string(),
        );
        ids.insert(package.subject.id.to_string(), subject.to_string());
        let identities = package
            .materials
            .iter()
            .map(|v| v.material.id.to_string())
            .chain(package.observations.iter().map(|v| v.id.to_string()))
            .chain(package.memories.iter().map(|v| v.id.to_string()))
            .chain(package.cognition.iter().map(|v| v.id.to_string()))
            .chain(package.source_regions.iter().map(|v| v.id.to_string()))
            .chain(package.representations.iter().map(|v| v.id.to_string()))
            .chain(package.derived_regions.iter().map(|v| v.id.to_string()))
            .chain(package.calls.iter().map(|v| v.id.to_string()));
        for id in identities {
            ids.entry(id)
                .or_insert_with(|| MaterialId::new().to_string());
        }
        for source in &package.purges {
            ids.entry(source.id.to_string())
                .or_insert_with(|| MaterialId::new().to_string());
        }
        Self {
            ids,
            namespace: namespace.clone(),
            subject: subject.clone(),
            sequence,
            recorded_at,
            placement: placement.clone(),
        }
    }
    pub fn id(&self, value: &str) -> Result<String> {
        self.ids.get(value).cloned().ok_or_else(|| {
            Error::invalid("package contains a reference outside its exported closure")
                .with_context("reference", value)
        })
    }
    pub fn value<T: Serialize + DeserializeOwned>(&self, source: &T) -> Result<T> {
        let mut value = serde_json::to_value(source)?;
        self.walk(&mut value, None)?;
        Ok(serde_json::from_value(value)?)
    }
    fn walk(&self, value: &mut Value, key: Option<&str>) -> Result<()> {
        match value {
            Value::Object(values) => {
                for (key, value) in values {
                    if matches!(key.as_str(), "metadata" | "extra" | "request" | "response") {
                        continue;
                    }
                    self.walk(value, Some(key))?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.walk(value, key)?;
                }
            }
            Value::String(text)
                if key
                    .is_some_and(|key| key == "id" || key.ends_with("_id") || key == "parent") =>
            {
                // UUID-bearing domain identity slots are remapped. Human text is unchanged.
                if let Some(mapped) = self.ids.get(text) {
                    *text = mapped.clone();
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn material(&self, source: &MediaOriginal) -> Result<MediaOriginal> {
        let mut value = self.value(source)?;
        value.material.namespace_id = self.namespace.clone();
        value.material.commit_seq = self.sequence;
        value.material.recorded_at = self.recorded_at.clone();
        Ok(value)
    }
    pub fn observation(&self, source: &Observation) -> Result<Observation> {
        let mut value: Observation = self.value(source)?;
        value.namespace_id = self.namespace.clone();
        value.subject_id = self.subject.clone();
        value.kind = ObservationKind::Import;
        value.origin = self
            .placement
            .conversation()
            .map(|conversation| ObservationOrigin {
                conversation_ref: Some(conversation.clone()),
                external_object_ref: None,
            });
        value.observed_at = None;
        value.recorded_at = self.recorded_at.clone();
        value.commit_seq = self.sequence;
        Ok(value)
    }
    pub fn memory(&self, source: &Memory) -> Result<Memory> {
        let mut value: Memory = self.value(source)?;
        value.namespace_id = self.namespace.clone();
        value.subject_id = self.subject.clone();
        value.recorded_at = self.recorded_at.clone();
        value.commit_seq = self.sequence;
        Ok(value)
    }
    pub fn cognition(&self, source: &StructuredObject) -> Result<StructuredObject> {
        let mut value: StructuredObject = self.value(source)?;
        value.namespace_id = self.namespace.clone();
        value.subject_id = self.subject.clone();
        value.recorded_at = self.recorded_at.clone();
        value.commit_seq = self.sequence;
        if let CognitiveContent::Journal(content) = &mut value.body {
            content.knowledge_cutoff = self.sequence;
        }
        if let CognitiveContent::MemorySemantics(content) = &mut value.body {
            if content.cognitive_role != CognitiveRole::Declarative {
                content.cognitive_role = CognitiveRole::Declarative;
                content
                    .qualifications
                    .push("Imported account of another subject's experience".into());
            }
            content.epistemic.class = EpistemicClass::Reported;
        }
        Ok(value)
    }
    pub fn representation(&self, source: &DerivedRepresentation) -> Result<DerivedRepresentation> {
        let mut value: DerivedRepresentation = self.value(source)?;
        value.namespace_id = self.namespace.clone();
        value.subject_id = self.subject.clone();
        if value.observation_id.is_some() {
            value.source_event_kind = Some(ObservationKind::Import);
        }
        value.recorded_at = self.recorded_at.clone();
        value.commit_seq = self.sequence;
        Ok(value)
    }
    pub fn region<T: Serialize + DeserializeOwned>(&self, source: &T) -> Result<T> {
        let mut value = serde_json::to_value(self.value(source)?)?;
        value["namespace_id"] = serde_json::to_value(&self.namespace)?;
        if value.get("subject_id").is_some() {
            value["subject_id"] = serde_json::to_value(&self.subject)?;
        }
        value["commit_seq"] = self.sequence.into();
        if value.get("recorded_at").is_some() {
            value["recorded_at"] = self.recorded_at.clone().into();
        }
        Ok(serde_json::from_value(value)?)
    }
    pub fn tombstone(&self, source: &SourceTombstone) -> Result<SourceTombstone> {
        let mut value: SourceTombstone = self.region(source)?;
        value.origin = source.origin.clone().or_else(|| {
            Some(SourceTombstoneOrigin {
                namespace_id: source.namespace_id.clone(),
                subject_id: source.subject_id.clone(),
                target: source.target.clone(),
                recorded_at: source.recorded_at.clone(),
                commit_seq: source.commit_seq,
            })
        });
        Ok(value)
    }
    pub fn vector_content(&self, source: &EmbeddingContent) -> Result<EmbeddingContent> {
        let mut value: EmbeddingContent = self.value(source)?;
        value.namespace_id = self.namespace.clone();
        value.subject_id = self.subject.clone();
        value.commit_seq = self.sequence;
        Ok(value)
    }
    pub fn vector_association(
        &self,
        source: &BundleEmbeddingAssociation,
    ) -> Result<BundleEmbeddingAssociation> {
        let mut value: BundleEmbeddingAssociation = self.value(source)?;
        value.acceptance = source.acceptance.clone();
        value.record.namespace_id = self.namespace.clone();
        value.record.subject_id = self.subject.clone();
        value.record.commit_seq = self.sequence;
        Ok(value)
    }
}
