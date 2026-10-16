// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use chrono::{DateTime, TimeDelta, Utc};
use nous_core::*;
use nous_kernel::NousRuntime;
use nous_runtime::{AutomaticContextRequest, InvocationScope, ManualCognitiveClock};
use nous_self::{AdoptSelf, SelfContent, SelfFacetContent, SelfScope};
use std::sync::Arc;
use test_support::*;

async fn subject_with_basis(runtime: &NousRuntime) -> (SubjectId, RevisionBasis) {
    let subject = runtime
        .subjects
        .create_subject(nous_subject::CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            metadata: serde_json::json!({}),
            capabilities: Some(nous_subject::SubjectCapabilities {
                memory: false,
                self_cognition: true,
                social_cognition: false,
                motivation: false,
            }),
            cognitive_seed: Some(nous_subject::CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: serde_json::json!({}),
            }),
        })
        .await
        .unwrap()
        .subject_id;
    let seed = runtime
        .subjects
        .cognitive_seed(subject, None)
        .await
        .unwrap();
    (
        subject,
        RevisionBasis::Seed(SeedBasisRef::new(seed.version.seed_version_id, "$").unwrap()),
    )
}

fn request<'a>(
    scope: &'a InvocationScope,
    max_items: usize,
    clock: &'a DomainStamp,
) -> AutomaticContextRequest<'a> {
    AutomaticContextRequest {
        clock,
        scope,
        max_items,
        work_context: None,
        materialize: true,
        max_bytes: 8192,
        ranked: &[],
    }
}

#[tokio::test]
async fn automatic_time_filter_preserves_submicrosecond_applicability_before_limits() {
    let (root, url, _postgres) = database().await;
    let now: DateTime<Utc> = "2026-10-15T14:00:00.123456100Z".parse().unwrap();
    let clock = Arc::new(ManualCognitiveClock::new(now));
    let runtime =
        NousRuntime::open_with_clock(runtime_options(&url, &root, false, false, false), clock)
            .await
            .unwrap();
    let (subject, basis) = subject_with_basis(&runtime).await;
    let now = runtime.cognition.read_clock(subject).await.unwrap().at;
    let service = runtime.require_self().unwrap();
    let later = now + TimeDelta::nanoseconds(100);
    let times = [
        TemporalExtent::Instant { at: now },
        TemporalExtent::Instant { at: later },
        TemporalExtent::Interval {
            start: Some(later),
            end: None,
        },
        TemporalExtent::Interval {
            start: None,
            end: Some(later),
        },
        TemporalExtent::Instant { at: now },
    ];
    let mut records = Vec::new();
    for (index, valid_time) in times.into_iter().enumerate() {
        records.push(
            service
                .adopt(AdoptSelf {
                    subject,
                    operation_id: OperationId::new(),
                    reason: "Exact applicability boundary regression".into(),
                    content: SelfContent::SelfFacet(SelfFacetContent {
                        valid_time_domain_ref: if index == 4 {
                            "time:unmapped".into()
                        } else {
                            format!("time:manual:{}", subject.0)
                        },
                        kind: if index < 2 { "identity" } else { "preference" }.into(),
                        domain_knowledge: None,
                        statement: format!("Exact temporal case {index}"),
                        epistemic_class: EpistemicClass::Narrative,
                        scope: SelfScope {
                            description: "general".into(),
                            ..Default::default()
                        },
                        stance: "adopted".into(),
                        exceptions: vec![],
                        basis: vec![basis.clone()],
                        valid_time,
                        producer: None,
                    }),
                })
                .await
                .unwrap(),
        );
    }
    let scope = InvocationScope {
        intent: "current applicability".into(),
        purpose: "expression".into(),
        conversation: "test".into(),
        entities: vec![],
        ..Default::default()
    };
    let read = nous_runtime::ContextResolver::source_read_view(
        &runtime,
        subject,
        &SourceReadScope {
            subject_id: Some(subject),
            source_refs: vec![basis.evidence_handle().reference],
            origin_conversation_refs: vec![],
        },
    )
    .await
    .unwrap();
    let captured_clock = runtime.cognition.read_clock(subject).await.unwrap();
    let one = service
        .automatic_context(subject, request(&scope, 1, &captured_clock), &read)
        .await
        .unwrap();
    assert_eq!(one.sources.len(), 1);
    assert_eq!(one.sources[0].reference, records[0].reference());
    let all = service
        .automatic_context(subject, request(&scope, 8, &captured_clock), &read)
        .await
        .unwrap();
    let selected = all
        .sources
        .iter()
        .map(|s| s.reference.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        selected,
        vec![records[0].reference(), records[3].reference()]
    );
}
