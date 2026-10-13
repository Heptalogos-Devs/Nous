// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::NousRuntime;
use nous_core::*;
use nous_runtime::{ContextResolver, ContextSource};

#[async_trait::async_trait]
impl ContextResolver for NousRuntime {
    async fn query_descriptors(
        &self,
        query: &CognitiveQuery,
        sources: &[nous_runtime::QuerySource],
        configuration: &nous_configuration::ConfigSnapshot,
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<QueryDescriptor>> {
        let (cognition, other): (Vec<_>, Vec<_>) = sources
            .iter()
            .cloned()
            .partition(|source| nous_memory::MemoryService::owns_content(&source.reference));
        let mut descriptors = if cognition.is_empty() {
            vec![]
        } else {
            self.require_memory()?
                .query_descriptors(
                    query.subject,
                    &cognition,
                    query.effort,
                    query.temporal_frame.clock_now,
                    configuration,
                    historical,
                )
                .await?
        };
        let (material, identities): (Vec<_>, Vec<_>) = other
            .into_iter()
            .map(|s| s.reference)
            .partition(|reference| {
                nous_runtime::CognitiveContributor::owns(&self.material, reference)
            });
        descriptors.extend(
            self.material
                .query_descriptors(query.subject, &material, historical)
                .await?,
        );
        descriptors.extend(
            self.store
                .identity_descriptors(query.subject, &identities, historical)
                .await?,
        );
        Ok(descriptors)
    }

    async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: usize,
        explicit: bool,
        now: chrono::DateTime<chrono::Utc>,
        configuration: &nous_configuration::ConfigSnapshot,
    ) -> Result<ContextSource> {
        if nous_memory::MemoryService::owns_content(reference) {
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
