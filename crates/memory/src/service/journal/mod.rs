//! Supported narrative Authority over settled Episode organization.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
mod mutations;
mod read;

#[derive(Debug, Clone, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum JournalPointRole {
    Summary,
    Outcome,
    Change,
    Decision,
    OpenQuestion,
    SalientEvent,
    Reflection,
}

impl JournalPointRole {
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalPoint {
    pub role: JournalPointRole,
    pub text: String,
    pub basis: Vec<RevisionBasis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalTarget {
    pub journal_id: JournalId,
    pub expected_revision: JournalRevisionId,
    pub expected_epoch: i64,
    pub intent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub expected_authority_seq: i64,
    pub target: Option<JournalTarget>,
    pub sources: Vec<EpisodePartitionSource>,
    pub title: Option<String>,
    pub narrative: String,
    pub points: Vec<JournalPoint>,
    pub producer: Option<ProducerSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRevision {
    pub header: CognitionRevisionHeader,
    pub revision_intent: Option<String>,
    pub title: Option<String>,
    pub temporal_scope: TemporalExtent,
    pub narrative: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalView {
    pub header: CognitionHeader,
    pub revision: JournalRevision,
    pub points: Vec<JournalPoint>,
    pub sources: Vec<CognitiveRef>,
}

impl MemoryService {
    pub(in crate::service) async fn wake_journal_revalidation_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        journal: JournalId,
        sequence: i64,
    ) -> Result<()> {
        self.cognition
            .wake_blocked_maintenance_in(
                tx,
                &nous_runtime::MaintenanceRequest {
                    subject,
                    kind: "journal_revalidate".into(),
                    scope_kind: "journal".into(),
                    scope_ref: journal.0.to_string(),
                    trigger_authority_seq: sequence,
                    due_at: self.cognition.now(subject),
                    priority: 30,
                },
            )
            .await
    }
}
