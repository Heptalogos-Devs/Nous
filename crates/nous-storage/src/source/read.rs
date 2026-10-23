// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see NOTICE.
use super::{SourceClosure, closure::closure, references::reference};
use crate::cognition::structured::read::object;
use crate::{Database, storage_error};
use nous_domain::*;

impl Database {
    pub async fn source_closure(
        &self,
        namespace: &NamespaceId,
        id: &CognitiveId,
        revision: Option<i64>,
    ) -> Result<SourceClosure> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let owner = object(&mut tx, namespace, id, revision).await?;
        let reference = reference(&owner);
        let result = closure(&mut tx, namespace, &owner.subject_id, reference).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
