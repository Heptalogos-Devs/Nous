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
        scope: &nous_runtime::InvocationScope,
        work_context: Option<Uuid>,
        materialize: bool,
    ) -> Result<Vec<nous_runtime::ContextSource>> {
        let (records, more) = self.list(subject, "self_facet", None, 100).await?;
        if more {
            return Err(Error::Unavailable(
                "Self selection exceeds its bounded current facet view".into(),
            ));
        }
        let now = self.runtime.now(subject);
        let mut selected = Vec::new();
        for record in records {
            let SelfContent::SelfFacet(facet) = &record.content else {
                continue;
            };
            let time = match &facet.valid_time {
                TemporalExtent::Unknown => true,
                TemporalExtent::Instant { at } => *at == now,
                TemporalExtent::Interval { start, end } => TimeInterval {
                    start: *start,
                    end: *end,
                }
                .contains(now),
            };
            if !record.is_current_available()
                || !time
                || (!facet.scope.purposes.is_empty()
                    && !facet.scope.purposes.contains(&scope.purpose))
                || facet
                    .scope
                    .work_context
                    .is_some_and(|w| Some(w.0) != work_context)
                || facet
                    .scope
                    .conversation
                    .as_ref()
                    .is_some_and(|c| c != &scope.conversation)
                || !facet
                    .scope
                    .entities
                    .iter()
                    .all(|e| scope.entities.contains(e))
            {
                continue;
            }
            let mut source = Self::record_context(&record, materialize);
            source.usage = if facet.kind == "identity" && facet.stance == "adopted" {
                "identity"
            } else {
                "situational_guidance"
            };
            source.required = source.usage == "identity";
            selected.push(source);
        }
        selected.sort_by_key(|s| !s.required);
        Ok(selected)
    }
}
