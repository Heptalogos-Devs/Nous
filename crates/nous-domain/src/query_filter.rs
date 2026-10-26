use crate::{
    CognitiveContent, CognitiveRole, ConversationRef, EpistemicClass, Error, FormationMode,
    MaterialId, Result, RetrievalRecord,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct QueryFilters {
    pub person_ids: Vec<String>,
    pub conversations: Vec<ConversationRef>,
    pub source_types: Vec<String>,
    pub tags_all: Vec<String>,
    pub tags_any: Vec<String>,
    pub excluded_source_types: Vec<String>,
    pub include_global_sources: bool,
    pub modalities: Vec<String>,
    pub cognitive_roles: Vec<CognitiveRole>,
    pub formation_modes: Vec<FormationMode>,
    pub evidence_classes: Vec<EpistemicClass>,
    pub all: Vec<QueryFilters>,
}
#[derive(Debug, Clone)]
pub struct QueryRootFacts {
    pub material_id: MaterialId,
    pub person_ids: Vec<String>,
    pub conversations: Vec<ConversationRef>,
    pub source_type: Option<String>,
    pub tags: Vec<String>,
    pub global_source: bool,
    pub modality: String,
}
impl QueryFilters {
    pub fn validate(&self) -> Result<()> {
        for (field, values) in [
            ("person_ids", &self.person_ids),
            ("source_types", &self.source_types),
            ("tags_all", &self.tags_all),
            ("tags_any", &self.tags_any),
            ("excluded_source_types", &self.excluded_source_types),
            ("modalities", &self.modalities),
        ] {
            let mut distinct = BTreeSet::new();
            for value in values {
                if value.trim().is_empty()
                    || value.len() > 1024
                    || value.chars().any(char::is_control)
                    || !distinct.insert(value)
                {
                    return Err(Error::invalid("query metadata selections need distinct nonempty tokens without control characters").with_context("field",field));
                }
            }
        }
        let mut conversations = BTreeSet::new();
        for value in &self.conversations {
            value.validate()?;
            if !conversations.insert(value) {
                return Err(Error::invalid(
                    "query conversation selections must be distinct",
                ));
            }
        }
        if self.modalities.iter().any(|value| {
            !matches!(
                value.as_str(),
                "text" | "image" | "audio" | "video" | "document" | "binary"
            )
        }) {
            return Err(Error::invalid(
                "query modality must name text/image/audio/video/document/binary",
            ));
        }
        distinct(&self.cognitive_roles, "cognitive_roles")?;
        distinct(&self.formation_modes, "formation_modes")?;
        distinct(&self.evidence_classes, "evidence_classes")?;
        for child in &self.all {
            child.validate()?;
        }
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        !self.requires_root() && !self.has_exclusions() && !self.needs_cognition()
    }
    fn requires_root(&self) -> bool {
        !self.person_ids.is_empty()
            || !self.conversations.is_empty()
            || !self.source_types.is_empty()
            || !self.tags_all.is_empty()
            || !self.tags_any.is_empty()
            || !self.modalities.is_empty()
            || self.all.iter().any(Self::requires_root)
    }
    pub fn matches(&self, roots: &[QueryRootFacts]) -> bool {
        if self.excludes(roots) {
            return false;
        }
        if !self.requires_root() {
            return true;
        }
        roots.iter().any(|root| self.matches_root(root))
    }
    fn excludes(&self, roots: &[QueryRootFacts]) -> bool {
        roots.iter().any(|root| {
            root.source_type
                .as_ref()
                .is_some_and(|kind| self.excluded_source_types.contains(kind))
        }) || self.all.iter().any(|child| child.excludes(roots))
    }
    fn matches_root(&self, root: &QueryRootFacts) -> bool {
        (self.person_ids.is_empty()
            || self
                .person_ids
                .iter()
                .any(|person| root.person_ids.contains(person)))
            && (self.conversations.is_empty()
                || self
                    .conversations
                    .iter()
                    .any(|conversation| root.conversations.contains(conversation))
                || (self.include_global_sources && root.global_source))
            && (self.source_types.is_empty()
                || root
                    .source_type
                    .as_ref()
                    .is_some_and(|kind| self.source_types.contains(kind)))
            && self.tags_all.iter().all(|tag| root.tags.contains(tag))
            && (self.tags_any.is_empty() || self.tags_any.iter().any(|tag| root.tags.contains(tag)))
            && (self.modalities.is_empty() || self.modalities.contains(&root.modality))
            && self.all.iter().all(|child| child.matches_root(root))
    }
    pub fn needs_cognition(&self) -> bool {
        !self.cognitive_roles.is_empty()
            || !self.formation_modes.is_empty()
            || !self.evidence_classes.is_empty()
            || self.all.iter().any(Self::needs_cognition)
    }
    pub fn needs_roots(&self) -> bool {
        self.requires_root() || self.has_exclusions()
    }
    fn has_exclusions(&self) -> bool {
        !self.excluded_source_types.is_empty() || self.all.iter().any(Self::has_exclusions)
    }
    pub fn and(&self, other: &Self) -> Self {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        Self {
            all: vec![self.clone(), other.clone()],
            ..Self::default()
        }
    }
    pub fn matches_record(&self, record: &RetrievalRecord) -> bool {
        match record {
            RetrievalRecord::Memory(memory) => {
                self.matches_cognition(None, Some(&memory.epistemic_status))
            }
            RetrievalRecord::Structured(object) => self.matches_cognition(Some(&object.body), None),
            RetrievalRecord::Media(_) => self.matches_cognition(None, None),
        }
    }
    pub fn matches_cognition(
        &self,
        body: Option<&CognitiveContent>,
        memory_status: Option<&str>,
    ) -> bool {
        let (role, mode, class) = match body {
            Some(CognitiveContent::MemorySemantics(value)) => (
                Some(value.cognitive_role),
                Some(value.formation_mode),
                Some(value.epistemic.class),
            ),
            Some(CognitiveContent::Schema(value)) => (
                None,
                Some(value.formation_mode),
                Some(value.epistemic.class),
            ),
            Some(CognitiveContent::Journal(value)) => (None, None, Some(value.perspective.class)),
            _ => (
                None,
                None,
                memory_status.and_then(|status| {
                    serde_json::from_value(serde_json::Value::String(status.into())).ok()
                }),
            ),
        };
        (self.cognitive_roles.is_empty()
            || role.is_some_and(|value| self.cognitive_roles.contains(&value)))
            && (self.formation_modes.is_empty()
                || mode.is_some_and(|value| self.formation_modes.contains(&value)))
            && (self.evidence_classes.is_empty()
                || class.is_some_and(|value| self.evidence_classes.contains(&value)))
            && self
                .all
                .iter()
                .all(|child| child.matches_cognition(body, memory_status))
    }
}
fn distinct<T: PartialEq>(values: &[T], field: &str) -> Result<()> {
    for (index, value) in values.iter().enumerate() {
        if values[..index].contains(value) {
            return Err(Error::invalid("query typed selections must be distinct")
                .with_context("field", field));
        }
    }
    Ok(())
}
