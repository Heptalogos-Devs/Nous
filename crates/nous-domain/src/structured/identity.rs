//! Exact typed identities of cognitive revisions.
use super::*;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveKind {
    Memory,
    MemorySemantics,
    Concept,
    Episode,
    Journal,
    Schema,
    Association,
}
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct CognitiveRef {
    pub kind: CognitiveKind,
    pub id: String,
    pub revision: i64,
}
impl CognitiveRef {
    pub fn memory(id: &MemoryId, revision: i64) -> Self {
        Self {
            kind: CognitiveKind::Memory,
            id: id.to_string(),
            revision,
        }
    }
    pub fn structured(kind: CognitiveKind, id: &CognitiveId, revision: i64) -> Self {
        Self {
            kind,
            id: id.to_string(),
            revision,
        }
    }
    pub fn validate(&self) -> Result<()> {
        validate_uuid(&self.id)?;
        if self.revision <= 0 {
            return Err(Error::invalid(
                "cognitive reference needs canonical UUID and exact positive revision",
            ));
        }
        Ok(())
    }
}

impl std::fmt::Display for CognitiveRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = serde_json::to_value(self.kind).map_err(|_| std::fmt::Error)?;
        write!(
            formatter,
            "{}:{}@{}",
            kind.as_str().ok_or(std::fmt::Error)?,
            self.id,
            self.revision
        )
    }
}

impl std::str::FromStr for CognitiveRef {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        let (kind, identity) = value
            .split_once(':')
            .ok_or_else(|| Error::invalid("cognitive reference requires kind:id@revision"))?;
        let (id, revision) = identity
            .rsplit_once('@')
            .ok_or_else(|| Error::invalid("cognitive reference requires an exact revision"))?;
        let reference = Self {
            kind: serde_json::from_value(serde_json::Value::String(kind.into()))
                .map_err(|_| Error::invalid("unknown cognitive reference kind"))?,
            id: id.into(),
            revision: revision
                .parse()
                .map_err(|_| Error::invalid("cognitive revision must be an integer"))?,
        };
        reference.validate()?;
        if reference.to_string() != value {
            return Err(Error::invalid(
                "cognitive reference must use canonical exact identity",
            ));
        }
        Ok(reference)
    }
}
