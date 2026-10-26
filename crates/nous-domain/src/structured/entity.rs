use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Person,
    Group,
    Organization,
    Place,
    Object,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalEntityIdentity {
    pub system: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityMention {
    pub target: SemanticTarget,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityAttribute {
    pub predicate: String,
    pub value: String,
    pub epistemic: EpistemicStance,
    pub validity: Option<TimeRange>,
    pub evidence: Vec<EvidenceLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityContent {
    pub entity_type: EntityType,
    pub label: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub identifiers: Vec<ExternalEntityIdentity>,
    pub mentions: Vec<EntityMention>,
    pub attributes: Vec<EntityAttribute>,
}

impl EntityContent {
    pub fn validate(&self) -> Result<()> {
        short(&self.label, "entity label")?;
        optional_text(&self.description, "entity description", 1_000_000)?;
        let mut aliases = BTreeSet::from([self.label.trim().to_lowercase()]);
        for alias in &self.aliases {
            short(alias, "entity alias")?;
            if !aliases.insert(alias.trim().to_lowercase()) {
                return Err(Error::invalid("entity repeats a label or alias"));
            }
        }
        let mut identifiers = BTreeSet::new();
        for identifier in &self.identifiers {
            short(&identifier.system, "external identity system")?;
            text(&identifier.key, "external identity key", 2048)?;
            if !identifiers.insert(identifier) {
                return Err(Error::invalid("entity repeats an external identity"));
            }
        }
        let mut mentions = BTreeSet::new();
        for mention in &self.mentions {
            mention.target.validate()?;
            short(&mention.role, "entity mention role")?;
            if !mentions.insert((&mention.target, &mention.role)) {
                return Err(Error::invalid("entity repeats a target participation role"));
            }
        }
        for attribute in &self.attributes {
            short(&attribute.predicate, "entity attribute predicate")?;
            text(&attribute.value, "entity attribute value", 65536)?;
            attribute.epistemic.validate()?;
            if let Some(validity) = &attribute.validity {
                validity.validate()?;
            }
            validate_links(&attribute.evidence)?;
            if !attribute
                .evidence
                .iter()
                .any(|link| link.polarity == EvidencePolarity::Support)
            {
                return Err(Error::invalid(
                    "entity attributes require their own positive evidence",
                ));
            }
        }
        if self.aliases.len() > 64
            || self.identifiers.len() > 64
            || self.mentions.len() > 1024
            || self.attributes.len() > 256
        {
            return Err(Error::invalid(
                "entity content exceeds its bounded semantic units",
            ));
        }
        Ok(())
    }
}
