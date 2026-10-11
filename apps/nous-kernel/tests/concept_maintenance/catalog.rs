// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

pub(super) async fn assert_wire_catalog(
    rt: &NousRuntime,
    subject: SubjectId,
    revisions: &[CognitiveRef],
    first: &MaintenanceNeed,
) {
    use k::kernel_maintenance_service_server::KernelMaintenanceService;
    use nous_protocol::nous::wave::kernel::v1alpha1 as k;
    let ts = |date: chrono::DateTime<chrono::Utc>| prost_types::Timestamp {
        seconds: date.timestamp(),
        nanos: i32::try_from(date.timestamp_subsec_nanos()).unwrap(),
    };
    let wire = k::MaintenanceNeed {
        need_id: first.need_id.to_string(),
        subject_id: subject.0.to_string(),
        kind: first.kind.clone(),
        scope_kind: first.scope_kind.clone(),
        scope_ref: first.scope_ref.clone(),
        trigger_authority_seq: first.trigger_authority_seq,
        trigger_revision: first.trigger_revision,
        due_at: Some(ts(first.due_at)),
        created_at: Some(ts(first.created_at)),
        updated_at: Some(ts(first.updated_at)),
        state: first.state.clone(),
        lease_token: first.lease_token.map(|t| t.to_string()),
        lease_until: first.lease_until.map(ts),
        ..Default::default()
    };
    let service = nous_kernel::transport::KernelService(rt.clone());
    let reply = KernelMaintenanceService::plan_maintenance(
        &service,
        tonic::Request::new(k::PlanMaintenanceRequest {
            claimed: Some(wire.clone()),
        }),
    )
    .await
    .unwrap()
    .into_inner();
    let wire_catalog = reply.concept_catalog.unwrap();
    assert_eq!(wire_catalog.max_suggestions, 4);
    assert_eq!(wire_catalog.references.len(), 1);
    assert_eq!(
        wire_catalog.references[0].reference.as_ref().unwrap().value,
        reference_parts(&revisions[0]).1
    );
    assert_eq!(wire_catalog.basis[0].key, "s0");
    assert!(
        reply
            .concept_model_input_json
            .unwrap()
            .contains("sourceContext")
    );
    let mut wrong_scope = wire;
    wrong_scope.scope_ref = reference_parts(&revisions[1]).1;
    assert!(
        KernelMaintenanceService::plan_maintenance(
            &service,
            tonic::Request::new(k::PlanMaintenanceRequest {
                claimed: Some(wrong_scope)
            })
        )
        .await
        .is_err()
    );
}

pub(super) async fn assert_schema_accretion(
    rt: &NousRuntime,
    subject: SubjectId,
    revisions: &[CognitiveRef],
) {
    let owner = rt.require_memory().unwrap();
    let schema = owner
        .create_schema(CreateSchemaInput {
            operation_id: OperationId::new(),
            subject,
            content: nous_memory::SchemaContent {
                producer: None,
                title: Some("Independent reviewer approval pattern".into()),
                structural_claim: "Release approval requires a recorded reviewer signoff".into(),
                applicability_scope: SchemaScope {
                    description: "The two independent release observations".into(),
                    aboutness: vec![],
                    tags: vec![],
                    valid_time: TemporalExtent::Unknown,
                },
                boundary_definition: "Does not establish approval policies for other projects"
                    .into(),
                formation_kind: SchemaFormationKind::Synthesized,
                evidence_links: revisions
                    .iter()
                    .map(|reference| SchemaEvidenceLinkInput {
                        role: SchemaEvidenceRole::Support,
                        basis: RevisionBasis::CognitionDependency(CognitionDependency {
                            epistemic_relation: None,
                            target_revision: reference.clone(),
                            basis_role: BasisRole::Direct,
                        }),
                    })
                    .collect(),
            },
        })
        .await
        .unwrap();
    let schema_ref = CognitiveRef::CognitiveSchemaRevision(schema.schema.current_revision_id);
    let signal = owner
        .accretion_signal(subject, &schema_ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(signal.attached_cognition, 2);
    assert_eq!(signal.independent_roots, 2);
}
