// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::NousRuntime;
use nous_core::*;
use nous_runtime::{ContextResolver, ContextSource};

#[async_trait::async_trait]
impl ContextResolver for NousRuntime {
    async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: usize,
        explicit: bool,
        now: chrono::DateTime<chrono::Utc>,
        configuration: &nous_configuration::ConfigSnapshot,
    ) -> Result<ContextSource> {
        if matches!(
            reference,
            CognitiveRef::Memory(_)
                | CognitiveRef::MemoryRevision(_)
                | CognitiveRef::CognitiveSchema(_)
                | CognitiveRef::CognitiveSchemaRevision(_)
                | CognitiveRef::Episode(_)
                | CognitiveRef::EpisodeRevision(_)
                | CognitiveRef::Journal(_)
                | CognitiveRef::JournalRevision(_)
                | CognitiveRef::Tag(_)
        ) {
            let policy = nous_memory::resolve_accessibility_policy(configuration)?;
            return self
                .require_memory()?
                .context_projection(
                    subject,
                    reference,
                    &policy,
                    now,
                    CognitiveEffort::Normal,
                    explicit,
                    max_bytes,
                )
                .await;
        }
        if !matches!(
            reference,
            CognitiveRef::Occurrence(_)
                | CognitiveRef::SourceRegion(_)
                | CognitiveRef::DerivedRepresentation(_)
                | CognitiveRef::DerivedRegion(_)
                | CognitiveRef::Artifact(_)
                | CognitiveRef::ExternalObject(_)
        ) {
            return self
                .cognition
                .runtime_context_source(subject, reference, max_bytes)
                .await;
        }
        self.material
            .context_source(subject, reference, max_bytes)
            .await
    }
}
