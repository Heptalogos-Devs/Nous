// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl SelfService {
    pub async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        materialize: bool,
    ) -> Result<nous_runtime::ContextSource> {
        if reference.domain() != CognitiveDomain::SelfCognition {
            return Err(Error::Invalid(
                "Self contribution requires its owned reference".into(),
            ));
        }
        let record = self
            .get(
                subject,
                reference.uuid().expect("Self UUID"),
                reference.is_cognition_revision(),
            )
            .await?;
        let actual = if reference.is_cognition_revision() {
            record.reference()
        } else {
            record.object_reference()
        };
        if actual != *reference {
            return Err(Error::Invalid("Self reference family mismatch".into()));
        }
        if !record.is_current_available() {
            return Err(Error::Unavailable(
                "Self is withdrawn, suppressed or needs revalidation".into(),
            ));
        }
        Ok(Self::record_context(&record, materialize))
    }
    pub(crate) fn record_context(
        record: &SelfRecord,
        materialize: bool,
    ) -> nous_runtime::ContextSource {
        let mut evidence = record
            .content
            .basis()
            .iter()
            .map(RevisionBasis::evidence_handle)
            .collect::<Vec<_>>();
        if let SelfContent::SelfFacet(facet) = &record.content {
            for exception in &facet.exceptions {
                for b in &exception.basis {
                    let mut value = b.evidence_handle();
                    value.basis_role = "counterexample".into();
                    evidence.push(value);
                }
            }
        }
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
                    "Historical Self revision (past understanding, not the current view):\n{text}"
                )
            }
        });
        nous_runtime::ContextSource {
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

impl SelfService {
    pub async fn automatic_context(
        &self,
        subject: SubjectId,
        input: nous_runtime::AutomaticContextRequest<'_>,
    ) -> Result<nous_runtime::AutomaticContext> {
        let scope = SelfScope {
            description: String::new(),
            purposes: vec![input.scope.purpose.clone()],
            entities: input.scope.entities.clone(),
            work_context: input.work_context.map(WorkContextId),
            conversation: Some(input.scope.conversation.clone()),
        };
        let selected = self
            .select(
                subject,
                SelfSelectionRequest {
                    scope: &scope,
                    targets: &[],
                    sources: &[],
                    ranked: input.ranked,
                    max_items: input.max_items,
                    max_bytes: input
                        .max_bytes
                        .saturating_mul(4)
                        .clamp(16384, 4 * 1024 * 1024),
                    automatic: true,
                },
            )
            .await?;
        let sources = selected
            .records
            .iter()
            .map(|record| {
                let mut source = Self::record_context(record, input.materialize);
                let SelfContent::SelfFacet(facet) = &record.content else {
                    unreachable!("automatic selection only selects facets")
                };
                source.required = facet.kind == "identity" && facet.stance == "adopted";
                source.usage = if source.required {
                    "identity"
                } else {
                    "situational_guidance"
                };
                source
            })
            .collect();
        Ok(nous_runtime::AutomaticContext {
            sources,
            degradation: selected.degradation,
        })
    }
}
