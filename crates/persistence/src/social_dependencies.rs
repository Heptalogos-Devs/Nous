// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use sqlx::Row;
use uuid::Uuid;
/// Mechanics for the Social owner's declared exact dependency edges. The caller
/// chooses the semantic source change; content/history are never rewritten.
impl AuthorityStore {
    pub async fn seed_source_reference_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        version: nous_core::CognitiveSeedVersionId,
    ) -> Result<nous_core::CognitiveRef> {
        let artifact=sqlx::query_scalar("SELECT artifact_id FROM cognitive_seed_versions WHERE subject_id=$1 AND seed_version_id=$2").bind(subject.0).bind(version.0).fetch_one(&mut **tx).await.map_err(database_error)?;
        Ok(nous_core::CognitiveRef::Artifact(nous_core::ArtifactId(
            artifact,
        )))
    }

    pub async fn social_source_roots_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        revision: Uuid,
    ) -> Result<Vec<nous_core::EvidenceRoot>> {
        let rows=sqlx::query("SELECT roots.root_key,roots.certainty FROM social_source_roots roots JOIN social_revisions r USING(revision_id) WHERE r.subject_id=$1 AND r.revision_id=$2").bind(subject.0).bind(revision).fetch_all(&mut **tx).await.map_err(database_error)?;
        rows.into_iter()
            .map(|r| {
                Ok(nous_core::EvidenceRoot {
                    root_key: r.try_get("root_key").map_err(database_error)?,
                    certainty: match r
                        .try_get::<String, _>("certainty")
                        .map_err(database_error)?
                        .as_str()
                    {
                        "known" => nous_core::EvidenceRootCertainty::Known,
                        _ => nous_core::EvidenceRootCertainty::OccurrenceOnly,
                    },
                })
            })
            .collect()
    }
}
