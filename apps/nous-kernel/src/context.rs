// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::NousRuntime;
use nous_core::*;
use nous_material::MaterializeRequest;
use nous_runtime::{ContextResolver, ContextSource, bounded_text, modality};

#[async_trait::async_trait]
impl ContextResolver for NousRuntime {
    #[expect(
        clippy::unnecessary_filter_map,
        reason = "context evidence selection preserves owner-specific support variants"
    )]
    async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: usize,
        explicit: bool,
    ) -> Result<ContextSource> {
        let memory = match reference {
            CognitiveRef::Memory(id) => {
                Some(self.require_memory()?.memory(subject, *id, None).await?)
            }
            CognitiveRef::MemoryRevision(id) => {
                Some(self.require_memory()?.revision(subject, *id).await?)
            }
            _ => None,
        };
        if let Some(memory) = memory {
            if !matches!(
                memory.object.acceptance_state,
                nous_memory::AcceptanceState::Accepted
            ) || !matches!(
                memory.object.integrity_state,
                nous_memory::IntegrityState::Valid
            ) || !matches!(
                memory.object.suppression_state,
                nous_memory::SuppressionState::Normal
            ) || !matches!(memory.object.purge_state, nous_memory::PurgeState::Normal)
            {
                return Err(Error::new(
                    nous_core::ErrorOwner::Kernel,
                    nous_core::ErrorCode::Unavailable,
                    "Memory is suppressed",
                ));
            }
            let level = self
                .require_memory()?
                .accessibility_level(
                    subject,
                    memory.object.memory_id,
                    self.cognition.now(subject),
                )
                .await?;
            if !nous_memory::accessibility_eligible(level, CognitiveEffort::Normal, explicit) {
                return Err(Error::new(
                    nous_core::ErrorOwner::Kernel,
                    nous_core::ErrorCode::Unavailable,
                    "Service unavailable",
                ));
            }

            let evidence = memory
                .basis
                .iter()
                .filter_map(|item| match item {
                    nous_memory::RevisionBasis::Evidence(value) => Some(EvidenceHandle {
                        epistemic_relation: value.epistemic_relation,
                        reference: value.cognitive_ref(),
                        basis_role: value.basis_role.as_str().into(),
                    }),
                    nous_memory::RevisionBasis::CognitionDependency(value) => {
                        Some(EvidenceHandle {
                            epistemic_relation: value.epistemic_relation,
                            reference: value.target_revision.clone(),
                            basis_role: value.basis_role.as_str().into(),
                        })
                    }
                    nous_memory::RevisionBasis::Seed(value) => Some(EvidenceHandle {
                        epistemic_relation: None,
                        reference: CognitiveRef::CognitiveSeedVersion(value.seed_version_id),
                        basis_role: "seed".into(),
                    }),
                })
                .collect::<Vec<_>>();
            return Ok(ContextSource {
                source_revision: Some(memory.revision.memory_revision_id),
                media_type: "text/plain".into(),
                text: (max_bytes > 0)
                    .then(|| bounded_text(memory.revision.representation_text, max_bytes)),
                authority: AuthorityClass::SubjectCognition,
                provenance: ProvenanceSummary {
                    source_count: evidence.len(),
                    producer_signatures: Vec::new(),
                    note: None,
                },
                evidence,
            });
        }
        if let CognitiveRef::Resource(resource) = reference {
            let descriptor = self
                .cognition
                .list_resources(subject)
                .await?
                .into_iter()
                .find(|view| view.descriptor.resource_ref == *resource)
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Kernel,
                        nous_core::ErrorCode::NotFound,
                        "Resource not found",
                    )
                })?
                .descriptor;
            return Ok(ContextSource {
                source_revision: None,
                media_type: "text/plain".into(),
                text: (max_bytes > 0).then(|| {
                    bounded_text(
                        descriptor
                            .display_label
                            .unwrap_or_else(|| resource.as_str().to_owned()),
                        max_bytes,
                    )
                }),
                authority: AuthorityClass::ResourceDescriptor,
                evidence: Vec::new(),
                provenance: ProvenanceSummary {
                    source_count: 0,
                    producer_signatures: Vec::new(),
                    note: Some("resource awareness".into()),
                },
            });
        }
        let material = self
            .material
            .materialize(
                subject,
                MaterializeRequest {
                    reference: reference.clone(),
                    byte_range: None,
                    max_bytes: (max_bytes.max(1) as u64).min(self.material.max_upload_bytes),
                    resource_handle: None,
                    resource: None,
                },
            )
            .await?;
        material_context(material, max_bytes)
    }
}

fn material_context(
    material: nous_material::MaterializedEvidence,
    max_bytes: usize,
) -> Result<ContextSource> {
    let text = if max_bytes > 0
        && matches!(
            modality(&material.media_type),
            Modality::Text | Modality::Structured
        ) {
        let end = match std::str::from_utf8(&material.bytes) {
            Ok(_) => material.bytes.len(),
            Err(error) => error.valid_up_to(),
        };
        Some(
            String::from_utf8(material.bytes[..end].to_vec()).map_err(|e| {
                Error::new(
                    nous_core::ErrorOwner::Kernel,
                    nous_core::ErrorCode::InternalFailure,
                    "Operation failed",
                )
                .with_source(e)
            })?,
        )
    } else {
        None
    };
    let evidence = material
        .provenance
        .into_iter()
        .map(|reference| EvidenceHandle {
            epistemic_relation: None,
            reference,
            basis_role: "source".into(),
        })
        .collect::<Vec<_>>();
    Ok(ContextSource {
        source_revision: None,
        media_type: material.media_type,
        text,
        authority: if material.producer.is_some() {
            AuthorityClass::Interpretation
        } else {
            AuthorityClass::Evidence
        },
        provenance: ProvenanceSummary {
            source_count: evidence.len(),
            producer_signatures: material
                .producer
                .and_then(|p| {
                    p.get("signature_hash")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                })
                .into_iter()
                .collect(),
            note: None,
        },
        evidence,
    })
}
