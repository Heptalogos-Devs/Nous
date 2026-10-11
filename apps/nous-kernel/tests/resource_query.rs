// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::Utc;
use nous_core::{CognitiveRef, Cue, TextCue};
use test_support::query::{query, subject};
use test_support::{database, open_runtime};
use uuid::Uuid;

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one frozen Resource cohort exercises forged results, single-use tickets and descriptor drift without repeated database setup"
)]
async fn resource_continuation_fences_identity_access_and_descriptor_drift() {
    use nous_core::{
        ExternalResourceRecord, ExternalResourceResult, ResourceRef, StableExternalRef,
    };
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let owner = subject(&runtime).await;
    let other = subject(&runtime).await;
    let resource = ResourceRef::new("resource:external").unwrap();
    let descriptor = nous_runtime::ResourceUpsert {
        resource_ref: resource.clone(),
        adapter_kind: "local-documents".into(),
        provider_profile: "external".into(),
        provider_locator: "dataset selector".into(),
        display_label: None,
        authority_class: "external".into(),
        coverage: serde_json::json!({}),
        query_dimensions: serde_json::json!({}),
        modalities: serde_json::json!([]),
        freshness_policy: serde_json::json!({}),
        access_cost_class: "network".into(),
        readiness: "ready".into(),
    };
    runtime
        .cognition
        .upsert_resource(owner, descriptor.clone())
        .await
        .unwrap();
    let mut request = query(owner);
    request.expression.cues = vec![Cue::Text(TextCue {
        text: "external fact".into(),
    })];
    assert!(!request.requests_resources());
    request.projection.domains = vec![nous_core::ResultDomain::Resource];
    assert!(request.requests_resources());
    request.result_need.limit = 1;
    let explicit = runtime.execute_query(request.clone(), None).await.unwrap();
    let (pool, ticket) = runtime
        .cognition
        .retain_query(explicit, test_support::query_lease())
        .unwrap();
    assert_eq!(pool.resource_actions.len(), 1);
    runtime
        .cognition
        .release_query(owner, ticket.unwrap())
        .unwrap();
    request.expression.constraints.current_authority = nous_core::CurrentAuthorityNeed::Required;
    for case in [
        "valid",
        "max_material",
        "oversized_material",
        "unknown_action",
        "foreign_resource",
        "excess",
        "denied",
        "stale",
        "drift",
    ] {
        let execution = runtime.execute_query(request.clone(), None).await.unwrap();
        let (pool, ticket) = runtime
            .cognition
            .retain_query(execution, test_support::query_lease())
            .unwrap();
        let ticket = ticket.unwrap();
        assert_eq!(pool.resource_actions.len(), 1);
        let action = &pool.resource_actions[0];
        let record = ExternalResourceRecord {
            resource_ref: resource.clone(),
            reference: StableExternalRef {
                provider_kind: "local-documents".into(),
                provider_profile: "external".into(),
                profile_digest: "a".repeat(64),
                resource_ref: resource.clone(),
                provider_resource_id: "dataset".into(),
                entry_id: "chunk".into(),
                entry_version: None,
                content_digest: "c6e1ab9c432c107aa308f5e1b6e0cc5eac5f87067d80fc8a792a76e724e53879"
                    .into(),
                source_locator: "opaque locator".into(),
                retrieved_at: Utc::now().to_rfc3339(),
                access_scope: action.provider_locator.clone(),
            },
            title: None,
            content: "external fact".into(),
            provider_rank: 1,
            provider_score: None,
            version_status: "current".into(),
            access_status: "allowed".into(),
        };
        let mut response = ExternalResourceResult {
            action_id: action.action_id,
            resource_ref: resource.clone(),
            status: "success".into(),
            records: vec![record],
        };
        match case {
            "max_material" | "oversized_material" => {
                use sha2::{Digest, Sha256};
                response.records[0].content =
                    "x".repeat(1048576 + usize::from(case == "oversized_material"));
                response.records[0].reference.content_digest =
                    Sha256::digest(response.records[0].content.as_bytes())
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect();
            }
            "unknown_action" => response.action_id = Uuid::new_v4(),
            "foreign_resource" => {
                response.records[0].reference.resource_ref =
                    ResourceRef::new("resource:foreign").unwrap()
            }
            "excess" => response.records.push(response.records[0].clone()),
            "denied" => response.records[0].access_status = "denied".into(),
            "stale" => response.records[0].version_status = "stale".into(),
            "drift" => {
                runtime
                    .cognition
                    .delete_resource(owner, resource.clone())
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let contributors = || nous_runtime::CognitiveContributors {
            material: Some(&runtime.material),
            shared: None,
            memory: Some(runtime.require_memory().unwrap()),
        };
        assert!(
            runtime
                .cognition
                .finalize_query(
                    other,
                    ticket,
                    Vec::new(),
                    vec![response.clone()],
                    contributors()
                )
                .await
                .is_err()
        );
        let outcome = runtime
            .cognition
            .finalize_query(owner, ticket, Vec::new(), vec![response], contributors())
            .await;
        if matches!(case, "valid" | "max_material") {
            let result = outcome.unwrap();
            assert_eq!(result.resource_records.len(), 1);
            assert!(result.resource_actions.is_empty());
            assert!(
                result
                    .results
                    .iter()
                    .all(|hit| !matches!(hit.reference, CognitiveRef::Memory(_)))
            );
        } else if case == "drift" {
            let result = outcome.unwrap();
            assert!(result.resource_records.is_empty());
            assert!(
                result
                    .degradation
                    .iter()
                    .any(|value| value.code == "resource_descriptor_changed_during_query")
            );
        } else {
            assert!(outcome.is_err(), "must reject {case}");
        }
        assert!(
            runtime
                .cognition
                .finalize_query(owner, ticket, Vec::new(), Vec::new(), contributors())
                .await
                .is_err()
        );
    }
}
