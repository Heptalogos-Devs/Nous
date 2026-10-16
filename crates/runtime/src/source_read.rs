//! A trusted Host's bounded source authorization; social scope never grants propagation.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use nous_core::*;
use std::collections::BTreeSet;
#[async_trait::async_trait]
pub trait SourceRootResolver: Send + Sync {
    async fn roots_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<BTreeSet<EvidenceRoot>>;
}
