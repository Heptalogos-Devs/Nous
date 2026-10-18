// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl SocialService {
    pub async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        materialize: bool,
    ) -> Result<nous_runtime::ContextSource> {
        if reference.domain() != CognitiveDomain::SocialCognition {
            return Err(Error::Invalid(
                "Social contribution requires its owned reference".into(),
            ));
        }
        let record = self
            .get(
                subject,
                reference.uuid().expect("Social UUID"),
                reference.is_cognition_revision(),
            )
            .await?;
        let actual = if reference.is_cognition_revision() {
            record.reference()
        } else {
            record.object_reference()
        };
        if actual != *reference {
            return Err(Error::Invalid("Social reference family mismatch".into()));
        }
        if !record.is_current_available() {
            return Err(Error::Unavailable(
                "Social is withdrawn, suppressed or needs revalidation".into(),
            ));
        }
        Ok(Self::record_context(&record, materialize))
    }
    pub fn record_context(record: &SocialRecord, materialize: bool) -> nous_runtime::ContextSource {
        let mut evidence = record
            .content
            .basis()
            .iter()
            .map(RevisionBasis::evidence_handle)
            .collect::<Vec<_>>();
        for e in &record.evidence {
            let mut value = e.basis.evidence_handle();
            value.basis_role = e.role.clone();
            evidence.push(value);
        }
        let text = materialize.then(|| record.context_text());
        nous_runtime::ContextSource {
            epistemic_class: None,
            domain_knowledge: None,
            temporal: Some(FreshnessDescriptor {
                temporal_evidence: TemporalEvidence::default(),
                valid_time: DomainTemporalExtent {
                    time_domain_ref: record.content.valid_time_domain_ref().to_owned(),
                    extent: record.content.valid_time().clone(),
                },
                formed_at: Some(DomainStamp {
                    time_domain_ref: record.formed_time_domain_ref.clone(),
                    at: record.formed_at,
                    recorded_at: record.formed_recorded_at,
                }),
                recorded_at: Some(record.recorded_at),
            }),
            reference: record.reference(),
            source_revision: Some(record.revision_id.to_string()),
            text,
            authority: AuthorityClass::SubjectCognition,
            evidence,
            usage: "task_evidence",
            required: false,
        }
    }
}

impl SocialRecord {
    /// Computed presentation only; immutable cognition revisions remain the sole stored authority.
    pub fn context_text(&self) -> String {
        let body = self.content.text();
        let body = if self.revision_id == self.current_revision_id {
            body
        } else {
            format!(
                "Historical Social revision (past understanding, not the current view):\n{body}"
            )
        };
        let mut lines = vec![
            body,
            format!(
                "精确修订：{}；对象epoch：{}",
                self.reference(),
                self.object_epoch
            ),
            format!(
                "状态：{} / {} / {}",
                self.acceptance_state, self.integrity_state, self.suppression_state
            ),
            format!("采用或修订理由：{}", self.reason),
            format!(
                "形成：{}；时间域：{}\n\n记录：{}",
                self.formed_at.to_rfc3339(),
                self.formed_time_domain_ref,
                self.recorded_at.to_rfc3339()
            ),
        ];
        lines.extend(
            self.evidence
                .iter()
                .map(|e| format!("补充依据（{}）：{}", e.role, e.basis.canonical_key())),
        );
        lines.join("\n\n")
    }
}
