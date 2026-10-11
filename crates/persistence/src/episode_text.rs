// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, TextProjectionFragment, database_error as db};
use nous_core::*;
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Caller-owned read budgets; persistence applies them without choosing policy.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EpisodeTextBudget {
    #[schemars(range(min = 1, max = 256))]
    pub max_members: usize,
    #[schemars(range(min = 1, max = 16384))]
    pub fragment_max_bytes: usize,
    #[schemars(range(min = 1, max = 1048576))]
    pub total_max_bytes: usize,
}

impl AuthorityStore {
    pub async fn episode_member_text_input(
        &self,
        subject: SubjectId,
        revisions: &[Uuid],
        budget: EpisodeTextBudget,
    ) -> Result<BTreeMap<Uuid, Vec<TextProjectionFragment>>> {
        self.episode_member_text_input_in_view(subject, revisions, budget, None)
            .await
    }
    pub async fn episode_member_text_input_in_view(
        &self,
        subject: SubjectId,
        revisions: &[Uuid],
        budget: EpisodeTextBudget,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<BTreeMap<Uuid, Vec<TextProjectionFragment>>> {
        let mut tx = self.begin().await?;
        let result =
            episode_member_text_input_in_view(&mut tx, subject, revisions, budget, view).await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
}

pub(crate) async fn episode_member_text_input_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    revisions: &[Uuid],
    budget: EpisodeTextBudget,
) -> Result<BTreeMap<Uuid, Vec<TextProjectionFragment>>> {
    episode_member_text_input_in_view(tx, subject, revisions, budget, None).await
}

pub(crate) async fn episode_member_text_input_in_view(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    revisions: &[Uuid],
    budget: EpisodeTextBudget,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<BTreeMap<Uuid, Vec<TextProjectionFragment>>> {
    let historical = view.is_some();
    let representations: Vec<Uuid> = view
        .into_iter()
        .flat_map(|v| &v.material_documents)
        .filter_map(|r| match r {
            CognitiveRef::DerivedRepresentation(id) => Some(id.0),
            _ => None,
        })
        .collect();
    let occurrences: Vec<Uuid> = view
        .into_iter()
        .flat_map(|v| &v.material_visibility)
        .filter_map(|r| match r {
            CognitiveRef::Occurrence(id) => Some(id.0),
            _ => None,
        })
        .collect();
    let rows = sqlx::query(r"
SELECT m.episode_revision_id,o.occurrence_id,
    (a.media_type LIKE 'text/%' OR a.media_type IN ('application/json','application/xml')) AS raw_text,
    d.derived_representation_id
FROM episode_revision_members m JOIN episode_revisions e USING(episode_revision_id)
JOIN observation_occurrences o ON o.occurrence_id=CASE WHEN m.ref_kind='occurrence' THEN m.ref_value::uuid END
LEFT JOIN artifacts a USING(artifact_id)
LEFT JOIN LATERAL (
    SELECT r.derived_representation_id
    FROM derived_representations r
    WHERE (($4 AND r.derived_representation_id=ANY($5::uuid[])) OR (NOT $4 AND EXISTS (SELECT 1 FROM coverage_needs c WHERE c.subject_id=$1 AND c.current_representation_id=r.derived_representation_id AND c.state='ready')))
        AND EXISTS (SELECT 1 FROM representation_source_regions($1,r.derived_representation_id) roots JOIN source_regions s USING(source_region_id) WHERE s.artifact_id=o.artifact_id)
        AND r.subject_id=$1 AND r.payload_text IS NOT NULL
        AND r.representation_kind IN ('extracted_text','ocr','transcript','audio_description','image_description','scene_description','summary')
        AND NOT EXISTS (
            SELECT 1 FROM representation_source_regions($1,r.derived_representation_id) roots
            JOIN source_regions original USING(source_region_id)
            WHERE original.artifact_id IS DISTINCT FROM o.artifact_id
        )
    ORDER BY r.created_at DESC,r.derived_representation_id LIMIT 1
) d ON true
WHERE e.subject_id=$1 AND o.subject_id=$1 AND m.episode_revision_id=ANY($2::uuid[]) AND m.ordinal<$3 AND (NOT $4 OR o.occurrence_id=ANY($6::uuid[]))
ORDER BY m.episode_revision_id,m.ordinal")
        .bind(subject.0).bind(revisions).bind(budget.max_members as i32).bind(historical).bind(representations).bind(occurrences).fetch_all(&mut **tx).await.map_err(db)?;
    let mut result = BTreeMap::<Uuid, Vec<TextProjectionFragment>>::new();
    for row in rows {
        let reference = if row.get::<Option<bool>, _>("raw_text") == Some(true) {
            CognitiveRef::Occurrence(OccurrenceId(row.get("occurrence_id")))
        } else if let Some(id) = row.get::<Option<Uuid>, _>("derived_representation_id") {
            CognitiveRef::DerivedRepresentation(DerivedRepresentationId(id))
        } else {
            continue;
        };
        let fragment = TextProjectionFragment { reference };
        result
            .entry(row.get("episode_revision_id"))
            .or_default()
            .push(fragment);
    }
    Ok(result)
}
