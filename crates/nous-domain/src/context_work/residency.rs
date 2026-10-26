use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextResidentState {
    Resident,
    Provisional,
    Evicted,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResident {
    pub target: RetrievalTarget,
    pub state: ContextResidentState,
    pub entry_reason: String,
    pub entered_at: String,
    pub last_meaningful_use_at: Option<String>,
    pub hold_until: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextResidentAdmission {
    pub target: RetrievalTarget,
    pub state: ContextResidentState,
    pub reason: String,
    pub hold_until: Option<String>,
    pub last_meaningful_use_at: Option<String>,
}
impl ContextResidentAdmission {
    pub fn validate(&self) -> Result<()> {
        self.target.validate()?;
        context_text(&self.reason, 512, true)?;
        if self.state == ContextResidentState::Evicted {
            return Err(Error::invalid("resident admission cannot request eviction"));
        }
        for value in [&self.hold_until, &self.last_meaningful_use_at]
            .into_iter()
            .flatten()
        {
            chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| Error::invalid("resident timestamps must be RFC3339"))?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextResidentChange {
    Admit {
        items: Vec<ContextResidentAdmission>,
    },
    Evict {
        targets: Vec<RetrievalTarget>,
    },
    Clear,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResidentOutcome {
    pub revision: i64,
    pub admitted: usize,
    pub evicted: usize,
}
