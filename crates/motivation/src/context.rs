// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl MotivationService {
    pub async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        materialize: bool,
    ) -> Result<nous_runtime::ContextSource> {
        if reference.domain() != CognitiveDomain::Motivation {
            return Err(Error::Invalid(
                "Motivation contribution requires its owned reference".into(),
            ));
        }
        let record = self
            .get(
                subject,
                reference.uuid().expect("Motivation UUID"),
                reference.is_cognition_revision(),
            )
            .await?;
        let actual = if reference.is_cognition_revision() {
            record.reference()
        } else {
            record.object_reference()
        };
        if actual != *reference {
            return Err(Error::Invalid(
                "Motivation reference family mismatch".into(),
            ));
        }
        if !record.is_current_available() {
            return Err(Error::Unavailable(
                "Motivation is withdrawn, suppressed or needs revalidation".into(),
            ));
        }
        Ok(Self::record_context(&record, materialize))
    }
    pub fn record_context(
        record: &MotivationRecord,
        materialize: bool,
    ) -> nous_runtime::ContextSource {
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
        let text = materialize.then(|| {
            let text = record.content.text();
            if record.revision_id == record.current_revision_id {
                text
            } else {
                format!(
                    "Historical Motivation revision (past understanding, not the current view):\n{text}"
                )
            }
        });
        nous_runtime::ContextSource {
            epistemic_class: None,
            domain_knowledge: None,
            temporal: Some(FreshnessDescriptor {
                temporal_evidence: TemporalEvidence::default(),
                valid_time: DomainTemporalExtent {
                    time_domain_ref: record.content.matter().valid_time_domain_ref.clone(),
                    extent: record.content.valid_time().clone(),
                },
                formed_at: Some(record.formed_stamp.clone()),
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
