// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::NousRuntime;
use nous_core::*;
use nous_runtime::{ContextResolver, ContextSource, bounded_text};

#[async_trait::async_trait]
impl ContextResolver for NousRuntime {
    async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: usize,
        explicit: bool,
    ) -> Result<ContextSource> {
        if matches!(
            reference,
            CognitiveRef::Memory(_) | CognitiveRef::MemoryRevision(_)
        ) {
            let policy = nous_memory::resolve_accessibility_policy(
                &self.configuration.snapshot_for_subject(subject)?,
            )?;
            let memory = self
                .require_memory()?
                .context_projection(
                    subject,
                    reference,
                    &policy,
                    self.cognition.now(subject),
                    CognitiveEffort::Normal,
                    explicit,
                    max_bytes,
                )
                .await?;
            return Ok(ContextSource {
                source_revision: Some(memory.source_revision),
                media_type: "text/plain".into(),
                text: memory.text,
                authority: AuthorityClass::SubjectCognition,
                provenance: ProvenanceSummary {
                    source_count: memory.evidence.len(),
                    producer_signatures: Vec::new(),
                    note: None,
                },
                evidence: memory.evidence,
            });
        }
        if let CognitiveRef::Resource(resource) = reference {
            let descriptor = self.cognition.resource(subject, resource).await?.descriptor;
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
        self.material
            .context_source(subject, reference, max_bytes)
            .await
    }
}
