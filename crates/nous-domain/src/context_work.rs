use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

identity!(WorkContextId);
identity!(ContextSessionId);
identity!(ContextPreparationId);
identity!(ContextProjectionId);
identity!(ContextAdmissionId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkContextState {
    Open,
    Paused,
    Ended,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkContextContent {
    pub purpose: String,
    #[serde(default)]
    pub context_text: String,
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
    #[serde(default)]
    pub constraints: Value,
    #[serde(default)]
    pub resume_conditions: Vec<String>,
    #[serde(default)]
    pub budget_summary: Value,
    #[serde(default)]
    pub anchors: Vec<RetrievalTarget>,
}
impl WorkContextContent {
    pub fn validate(&self) -> Result<()> {
        text(&self.purpose, 8192, true)?;
        text(&self.context_text, 65536, false)?;
        for values in [&self.unresolved_questions, &self.resume_conditions] {
            if values.len() > 64 {
                return Err(Error::invalid(
                    "WorkContext has too many questions or resume conditions",
                ));
            }
            for value in values {
                text(value, 2048, true)?;
            }
        }
        if serde_json::to_vec(&self.constraints)?.len() > 65536
            || serde_json::to_vec(&self.budget_summary)?.len() > 16384
        {
            return Err(Error::invalid(
                "WorkContext structured constraints or budget summary exceeds its bound",
            ));
        }
        targets(&self.anchors, 256)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkContext {
    pub id: WorkContextId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub revision: i64,
    pub state: WorkContextState,
    pub content: WorkContextContent,
    pub created_at: String,
    pub recorded_at: String,
    pub ended_at: Option<String>,
    pub commit_seq: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkContextRequest {
    pub subject_id: SubjectId,
    pub content: WorkContextContent,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkContextChange {
    Edit { content: WorkContextContent },
    Pin { anchors: Vec<RetrievalTarget> },
    Unpin { anchors: Vec<RetrievalTarget> },
    ClearText,
    ClearAnchors,
    Pause,
    Resume,
    End,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenContextSessionRequest {
    pub subject_id: SubjectId,
    pub native_session: String,
    pub parent_session: Option<ContextSessionId>,
    pub history_generation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextSessionChange {
    pub active_work_context: Option<WorkContextId>,
    #[serde(default)]
    pub close: bool,
}
mod residency;
pub use residency::*;
mod epistemic;
pub use epistemic::*;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSession {
    pub id: ContextSessionId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub native_session: String,
    pub parent_session: Option<ContextSessionId>,
    pub history_generation: String,
    pub active_work_context: Option<WorkContextId>,
    pub revision: i64,
    pub history_invalidated: bool,
    #[serde(default)]
    pub residents: Vec<ContextResident>,
    pub created_at: String,
    pub recorded_at: String,
    pub closed_at: Option<String>,
    pub commit_seq: i64,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextView {
    #[default]
    Subject,
    Execution,
    Review,
    Maintenance,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextStewardMode {
    Off,
    #[default]
    Optional,
    Required,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConsumerPolicy {
    pub max_items: usize,
    pub max_text_bytes: usize,
    pub max_residents: usize,
    pub allowed_kinds: Vec<CognitiveKind>,
    pub allow_media: bool,
    pub steward: ContextStewardMode,
}
impl Default for ContextConsumerPolicy {
    fn default() -> Self {
        Self {
            max_items: 64,
            max_text_bytes: 65536,
            max_residents: 64,
            allowed_kinds: Vec::new(),
            allow_media: true,
            steward: ContextStewardMode::Optional,
        }
    }
}
impl ContextConsumerPolicy {
    pub fn validate(&self) -> Result<()> {
        if !(1..=256).contains(&self.max_items)
            || !(1..=1024 * 1024).contains(&self.max_text_bytes)
            || self.max_residents > 256
        {
            return Err(Error::invalid("context consumer limits are out of range"));
        }
        let kinds = self.allowed_kinds.iter().collect::<BTreeSet<_>>();
        if kinds.len() != self.allowed_kinds.len() {
            return Err(Error::invalid("context consumer repeats a cognition kind"));
        }
        Ok(())
    }
    pub fn allows(&self, target: &RetrievalTarget) -> bool {
        match target {
            RetrievalTarget::Cognition(reference) => {
                self.allowed_kinds.is_empty() || self.allowed_kinds.contains(&reference.kind)
            }
            RetrievalTarget::Media(_) => self.allow_media,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextUsage {
    pub purpose: String,
    pub intent: String,
    pub audience: String,
    #[serde(default)]
    pub view: ContextView,
    pub role: Option<String>,
    pub activity_ref: Option<String>,
    pub world_id: Option<String>,
    pub world_time: Option<TimePoint>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextCursor {
    pub track_id: String,
    pub epoch_id: String,
    pub revision: u64,
    pub projection_id: ContextProjectionId,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextPrepareRequest {
    pub subject_id: SubjectId,
    pub source_read: Option<SourceReadScope>,
    pub session_id: ContextSessionId,
    pub consumer_id: String,
    pub instance_id: String,
    pub usage: ContextUsage,
    pub max_items: usize,
    pub max_text_bytes: usize,
    pub query: Option<QueryRequest>,
    #[serde(default)]
    pub current_sources: Vec<RetrievalTarget>,
    pub known_cursor: Option<ContextCursor>,
}
impl ContextPrepareRequest {
    pub fn validate(&self) -> Result<()> {
        self.subject_id.validate()?;
        if let Some(scope) = &self.source_read {
            scope.validate()?;
            if scope.subject_id != self.subject_id {
                return Err(Error::invalid(
                    "context source-read scope belongs to another subject",
                ));
            }
        }
        self.session_id.validate()?;
        for value in [
            &self.consumer_id,
            &self.instance_id,
            &self.usage.purpose,
            &self.usage.audience,
        ] {
            text(value, 512, true)?;
        }
        text(&self.usage.intent, 8192, true)?;
        for value in [
            &self.usage.role,
            &self.usage.activity_ref,
            &self.usage.world_id,
        ]
        .into_iter()
        .flatten()
        {
            text(value, 1024, true)?;
        }
        if let Some(point) = &self.usage.world_time {
            point.validate()?;
        }
        if self.max_items == 0
            || self.max_items > 256
            || self.max_text_bytes == 0
            || self.max_text_bytes > 1024 * 1024
        {
            return Err(Error::invalid(
                "context contribution budget is out of range",
            ));
        }
        targets(&self.current_sources, 128)?;
        if let Some(cursor) = &self.known_cursor {
            cursor.projection_id.validate()?;
            text(&cursor.track_id, 128, true)?;
            text(&cursor.epoch_id, 128, true)?;
            if cursor.revision == 0 {
                return Err(Error::invalid(
                    "context cursor requires a positive applied revision",
                ));
            }
        }
        if let Some(query) = &self.query {
            query.validate()?;
        }
        if self
            .query
            .as_ref()
            .is_some_and(|query| query.subject_id != self.subject_id)
        {
            return Err(Error::invalid("context query belongs to another subject"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextAuthority {
    RecordedCognition,
    SourceMaterial,
    TaskDirective,
    StewardInterpretation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSegment {
    pub key: String,
    pub text: String,
    pub sources: Vec<RetrievalTarget>,
    pub required: bool,
    pub dependencies: Vec<RetrievalTarget>,
    pub authority: ContextAuthority,
    pub provenance: Value,
    pub epistemic: Option<ContextEpistemic>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextProjection {
    pub source_read: Vec<SourceReadScope>,
    pub id: ContextProjectionId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub session_id: ContextSessionId,
    pub consumer_id: String,
    pub instance_id: String,
    pub usage: ContextUsage,
    pub config_digest: String,
    pub session_revision: i64,
    pub work_context: Option<WorkContext>,
    pub source_watermark: i64,
    pub segments: Vec<ContextSegment>,
    pub epistemic_view: ContextEpistemicView,
    pub cursor: ContextCursor,
    pub steward_call: Option<ModelCallId>,
    pub degradation: Vec<String>,
    pub recorded_at: String,
    pub commit_seq: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPatchKind {
    Reset,
    Append,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedContextResponse {
    pub kind: ContextPatchKind,
    pub cursor: ContextCursor,
    pub projection: ContextProjection,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPreparation {
    pub id: ContextPreparationId,
    pub job: JobRecord,
    pub request: ContextPrepareRequest,
    pub config_digest: Option<String>,
    pub catalog: Vec<ContextSegment>,
    pub work_context: Option<WorkContext>,
    pub policy: Option<ContextConsumerPolicy>,
    pub session_revision: Option<i64>,
    pub source_watermark: Option<i64>,
    pub steward_job_id: Option<String>,
    pub response: Option<ManagedContextResponse>,
    pub degradation: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostInputMeasurement {
    pub input_tokens: u64,
    pub reserved_output_tokens: u64,
    pub context_window_tokens: u64,
    pub source: String,
}
impl HostInputMeasurement {
    pub fn validate(&self) -> Result<()> {
        text(&self.source, 256, true)?;
        if self.context_window_tokens == 0
            || self
                .input_tokens
                .checked_add(self.reserved_output_tokens)
                .is_none_or(|tokens| tokens > self.context_window_tokens)
        {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "host-reported complete input exceeds its context window",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextAdmitRequest {
    pub preparation_id: ContextPreparationId,
    pub invocation_id: String,
    pub applied_segment_ids: Vec<String>,
    pub rendered_digest: String,
    pub history_generation: String,
    pub measurement: HostInputMeasurement,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextAdmission {
    pub id: ContextAdmissionId,
    pub projection_id: ContextProjectionId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub session_id: ContextSessionId,
    pub consumer_id: String,
    pub instance_id: String,
    pub invocation_id: String,
    pub rendered_digest: String,
    pub history_generation: String,
    pub segments: Vec<String>,
    pub sources: Vec<RetrievalTarget>,
    pub recorded_at: String,
    pub commit_seq: i64,
    pub dependencies: Vec<RetrievalTarget>,
    pub measurement: HostInputMeasurement,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextUseKind {
    Referenced,
    ActedOn,
    ResultSupported,
    ResultRefuted,
    Corrected,
    Pinned,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextUse {
    pub event_id: String,
    pub target: RetrievalTarget,
    pub kind: ContextUseKind,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextOutcome {
    Completed,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextReportRequest {
    pub admission_id: ContextAdmissionId,
    pub invocation_id: String,
    pub outcome: ContextOutcome,
    pub occurred_at: String,
    #[serde(default)]
    pub uses: Vec<ContextUse>,
}
impl ContextReportRequest {
    pub fn validate(&self) -> Result<()> {
        self.admission_id.validate()?;
        text(&self.invocation_id, 512, true)?;
        chrono::DateTime::parse_from_rfc3339(&self.occurred_at)
            .map_err(|_| Error::invalid("context outcome requires an actual UTC timestamp"))?;
        if self.uses.len() > 256 {
            return Err(Error::invalid("too many context usage events"));
        }
        let mut ids = BTreeSet::new();
        for event in &self.uses {
            text(&event.event_id, 512, true)?;
            event.target.validate()?;
            if !ids.insert(&event.event_id) {
                return Err(Error::invalid("repeated context event id"));
            }
        }
        Ok(())
    }
}
pub fn context_text(value: &str, max: usize, required: bool) -> Result<()> {
    text(value, max, required)
}
fn text(value: &str, max: usize, required: bool) -> Result<()> {
    if value.len() > max || value.contains('\0') || (required && value.trim().is_empty()) {
        return Err(Error::invalid(
            "context text field is empty or exceeds its bound",
        ));
    }
    Ok(())
}
fn targets(values: &[RetrievalTarget], max: usize) -> Result<()> {
    if values.len() > max {
        return Err(Error::invalid("context reference bound exceeded"));
    }
    let mut seen = BTreeSet::new();
    for value in values {
        value.validate()?;
        if !seen.insert(value) {
            return Err(Error::invalid("duplicate context reference"));
        }
    }
    Ok(())
}
