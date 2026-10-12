// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;

impl MemoryService {
    pub(crate) async fn source_objects_for_revisions(
        &self,
        subject: SubjectId,
        revisions: &[uuid::Uuid],
    ) -> Result<std::collections::HashMap<uuid::Uuid, Vec<CognitiveRef>>> {
        use sqlx::Row;
        let rows = sqlx::query("WITH evidence AS (SELECT e.*,r.subject_id FROM memory_revision_evidence e JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=$1 AND e.memory_revision_id=ANY($2::uuid[])), roots AS (SELECT e.memory_revision_id,o.external_object_ref FROM evidence e JOIN observation_occurrences o USING(occurrence_id) UNION SELECT e.memory_revision_id,o.external_object_ref FROM evidence e LEFT JOIN derived_regions dr ON dr.derived_region_id=e.derived_region_id CROSS JOIN LATERAL representation_source_regions(e.subject_id,COALESCE(e.derived_representation_id,dr.derived_representation_id)) sr JOIN source_regions r ON r.source_region_id=sr.source_region_id JOIN observation_occurrences o ON o.subject_id=e.subject_id AND o.artifact_id=r.artifact_id) SELECT DISTINCT memory_revision_id,external_object_ref FROM roots WHERE external_object_ref IS NOT NULL ORDER BY memory_revision_id,external_object_ref")
            .bind(subject.0).bind(revisions).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut result: std::collections::HashMap<uuid::Uuid, Vec<CognitiveRef>> =
            std::collections::HashMap::new();
        for row in rows {
            result
                .entry(row.try_get("memory_revision_id").map_err(db)?)
                .or_default()
                .push(CognitiveRef::ExternalObject(ObjectRef::new(
                    row.try_get::<String, _>("external_object_ref")
                        .map_err(db)?,
                )?));
        }
        Ok(result)
    }
}
