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
                    &cognition
                        .iter()
                        .map(|s| {
                            (
                                s.reference.clone(),
                                !s.is_runtime() || s.origin == "work_context",
                            )
                        })
                        .collect::<Vec<_>>(),
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

    async fn context_sources(
        &self,
        subject: SubjectId,
        references: &[(CognitiveRef, bool)],
        max_bytes: usize,
        now: chrono::DateTime<chrono::Utc>,
        configuration: &nous_configuration::ConfigSnapshot,
    ) -> Result<Vec<Result<ContextSource>>> {
        let mut result = (0..references.len())
            .map(|_| Err(Error::NotFound("context source not found".into())))
            .collect::<Vec<_>>();
        let (cognition, other): (Vec<_>, Vec<_>) = references
            .iter()
            .enumerate()
            .partition(|(_, (reference, _))| nous_memory::MemoryService::owns_content(reference));
        if !cognition.is_empty() {
            let policy = nous_memory::resolve_accessibility_policy(configuration)?;
            let sources = self
                .require_memory()?
                .context_sources(
                    subject,
                    &cognition
                        .iter()
                        .map(|(_, r)| (*r).clone())
                        .collect::<Vec<_>>(),
                    &policy,
                    now,
                    max_bytes,
                )
                .await?;
            for ((index, _), source) in cognition.into_iter().zip(sources) {
                result[index] = source;
            }
        }
        let (material, other): (Vec<_>, Vec<_>) =
            other.into_iter().partition(|(_, (reference, _))| {
                matches!(reference, CognitiveRef::ExternalObject(_))
                    || nous_runtime::CognitiveContributor::owns(&self.material, reference)
            });
        if !material.is_empty() {
            let sources = self
                .material
                .context_sources(
                    subject,
                    &material
                        .iter()
                        .map(|(_, (r, _))| r.clone())
                        .collect::<Vec<_>>(),
                    max_bytes,
                )
                .await?;
            for ((index, _), source) in material.into_iter().zip(sources) {
                result[index] = source;
            }
        }
        for (index, (reference, _)) in other {
            result[index] = self
                .cognition
                .runtime_context_source(subject, reference, max_bytes)
                .await;
        }
        Ok(result)
    }
}
