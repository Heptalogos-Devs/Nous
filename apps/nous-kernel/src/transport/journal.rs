// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{JournalId, JournalRevisionId, OperationId, Result};

pub(super) fn journal_revision_proto(value: nous_memory::JournalView) -> p::JournalRevision {
    let revision = value.revision;
    p::JournalRevision {
        journal_revision_id: revision.journal_revision_id.0.to_string(),
        journal_id: revision.journal_id.0.to_string(),
        subject_id: revision.subject_id.0.to_string(),
        revision_no: revision.revision_no,
        parent_revision_id: revision.parent_revision_id.map(|id| id.0.to_string()),
        revision_intent: revision.revision_intent,
        title: revision.title,
        temporal_scope: Some(temporal_proto(&revision.temporal_scope)),
        narrative: revision.narrative,
        formed_at: Some(formation_proto(
            &revision.formed_time_domain_ref,
            revision.formed_at,
            revision.recorded_at,
        )),
        recorded_at: Some(timestamp(revision.recorded_at)),
        producer_signature_id: revision.producer_signature_id.map(|id| id.to_string()),
        points: value
            .points
            .into_iter()
            .enumerate()
            .map(|(ordinal, point)| p::JournalPoint {
                ordinal: ordinal as i32,
                role: point.role.as_str().into(),
                text: point.text,
                basis: point.basis.into_iter().map(basis_proto).collect(),
            })
            .collect(),
        sources: value.sources.into_iter().map(to_ref).collect(),
    }
}

fn journal_proto(value: nous_memory::JournalView) -> p::Journal {
    let object = &value.object;
    let current = object.current_revision_id == value.revision.journal_revision_id;
    p::Journal {
        journal_id: object.journal_id.0.to_string(),
        subject_id: object.subject_id.0.to_string(),
        current_revision_id: object.current_revision_id.0.to_string(),
        object_epoch: object.object_epoch,
        acceptance_state: enum_name(object.acceptance_state),
        integrity_state: enum_name(object.integrity_state),
        suppression_state: enum_name(object.suppression_state),
        created_at: Some(timestamp(object.created_at)),
        current_revision: current.then(|| journal_revision_proto(value)),
    }
}

pub(super) fn journal_response(value: nous_memory::JournalView) -> p::JournalResponse {
    p::JournalResponse {
        journal: Some(journal_proto(value)),
        degradation: vec![],
    }
}

impl KernelService {
    pub(super) async fn get_journal(&self, input: p::ObjectRequest) -> Result<p::JournalResponse> {
        Ok(journal_response(
            self.require_memory()?
                .journal(
                    SubjectId(id(&input.subject_id)?),
                    JournalId(id(&input.id)?),
                    None,
                )
                .await?,
        ))
    }
    pub(super) async fn get_journal_revision(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::JournalRevision> {
        Ok(journal_revision_proto(
            self.require_memory()?
                .journal_revision(
                    SubjectId(id(&input.subject_id)?),
                    JournalRevisionId(id(&input.id)?),
                )
                .await?,
        ))
    }
    pub(super) async fn list_journals(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListJournalsResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let scope = format!("journals:{}:{}", subject.0, input.status);
        let (limit, last) = page(input.page, &scope)?;
        let mut views = self
            .require_memory()?
            .journal_page(subject, None, limit + 1, last, &input.status)
            .await?;
        let next = if views.len() as i64 > limit {
            views.pop();
            next_token(
                &scope,
                views
                    .last()
                    .expect("nonempty Journal page")
                    .object
                    .journal_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListJournalsResponse {
            items: views.into_iter().map(journal_proto).collect(),
            next_page_token: next,
        })
    }
    pub(super) async fn list_journal_revisions(
        &self,
        input: p::ListJournalRevisionsRequest,
    ) -> Result<p::ListJournalRevisionsResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let journal = JournalId(id(&input.journal_id)?);
        let scope = format!("journal-history:{}:{}", subject.0, journal.0);
        let (limit, last) = page(input.page, &scope)?;
        let mut views = self
            .require_memory()?
            .journal_page(subject, Some(journal), limit + 1, last, "")
            .await?;
        let next = if views.len() as i64 > limit {
            views.pop();
            next_token(
                &scope,
                views
                    .last()
                    .expect("nonempty Journal revision page")
                    .revision
                    .journal_revision_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListJournalRevisionsResponse {
            items: views.into_iter().map(journal_revision_proto).collect(),
            next_page_token: next,
        })
    }
    async fn journal_lifecycle(
        &self,
        input: p::JournalMutationRequest,
        action: nous_memory::LifecycleAction,
    ) -> Result<p::JournalResponse> {
        Ok(journal_response(
            self.require_memory()?
                .mutate_journal_lifecycle(
                    SubjectId(id(&input.subject_id)?),
                    JournalId(id(&input.journal_id)?),
                    OperationId(id(&input.operation_id)?),
                    input.expected_object_epoch,
                    action,
                )
                .await?,
        ))
    }
    pub(super) async fn suppress_journal(
        &self,
        input: p::JournalMutationRequest,
    ) -> Result<p::JournalResponse> {
        self.journal_lifecycle(input, nous_memory::LifecycleAction::Suppress)
            .await
    }
    pub(super) async fn restore_journal(
        &self,
        input: p::JournalMutationRequest,
    ) -> Result<p::JournalResponse> {
        self.journal_lifecycle(input, nous_memory::LifecycleAction::Restore)
            .await
    }
    pub(super) async fn withdraw_journal(
        &self,
        input: p::JournalMutationRequest,
    ) -> Result<p::JournalResponse> {
        self.journal_lifecycle(input, nous_memory::LifecycleAction::Withdraw)
            .await
    }
    pub(super) async fn reaccept_journal(
        &self,
        input: p::JournalMutationRequest,
    ) -> Result<p::JournalResponse> {
        self.journal_lifecycle(input, nous_memory::LifecycleAction::Reaccept)
            .await
    }
    pub(super) async fn purge_journal(&self, input: p::JournalMutationRequest) -> Result<()> {
        self.require_memory()?
            .purge_journal(
                SubjectId(id(&input.subject_id)?),
                JournalId(id(&input.journal_id)?),
                OperationId(id(&input.operation_id)?),
                input.expected_object_epoch,
            )
            .await
    }
}
