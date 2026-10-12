// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error as db};
use nous_core::*;
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryDescriptor {
    pub reference: CognitiveRef,
    pub text: String,
}
impl AuthorityStore {
    /// One bounded catalog read, independent of candidate retrieval.
    pub async fn query_descriptors(
        &self,
        subject: SubjectId,
        refs: &[CognitiveRef],
    ) -> Result<Vec<QueryDescriptor>> {
        if refs.len() > 256 {
            return Err(Error::Invalid("query descriptor catalog exceeded".into()));
        }
        let (kinds, values): (Vec<_>, Vec<_>) = refs.iter().map(reference_parts).unzip();
        let rows = sqlx::query(r#"
WITH requested AS (SELECT * FROM unnest($2::text[],$3::text[]) AS r(kind,value)),
catalog AS (
 SELECT r.kind,r.value,left(v.display_name || CASE WHEN cardinality(v.aliases)>0 THEN ' (' || array_to_string(v.aliases,', ') || ')' ELSE '' END,2048) AS text
 FROM requested r JOIN lexical_bindings b ON b.object_kind=r.kind AND b.canonical_ref=r.value JOIN lexical_visibility v USING(lexical_ref)
 WHERE r.kind IN ('entity','resource','external_object') AND v.subject_id=$1 AND b.tombstoned_at IS NULL
 UNION ALL
 SELECT r.kind,r.value,left(COALESCE(t.title||': ','')||t.structural_claim||' Scope: '||t.applicability_description||' Boundary: '||t.boundary_definition,2048)
 FROM requested r JOIN cognitive_schemas o ON r.kind='cognitive_schema' AND o.schema_id::text=r.value JOIN cognitive_schema_revisions t ON t.schema_revision_id=o.current_revision_id
 WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 UNION ALL
 SELECT r.kind,r.value,left(COALESCE(t.title||': ','')||t.representation_text,2048)
 FROM requested r JOIN memory_revisions t ON r.kind='memory_revision' AND t.memory_revision_id::text=r.value JOIN memory_objects o USING(memory_id)
 WHERE t.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 UNION ALL
 SELECT r.kind,r.value,left(COALESCE(t.title||': ','')||t.structural_claim||' Scope: '||t.applicability_description||' Boundary: '||t.boundary_definition,2048)
 FROM requested r JOIN cognitive_schema_revisions t ON r.kind='cognitive_schema_revision' AND t.schema_revision_id::text=r.value JOIN cognitive_schemas o USING(schema_id)
 WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 UNION ALL
 SELECT r.kind,r.value,left(COALESCE(t.title||': ','')||t.boundary_explanation,2048)
 FROM requested r JOIN episode_revisions t ON r.kind='episode_revision' AND t.episode_revision_id::text=r.value JOIN episode_objects o USING(episode_id)
 WHERE t.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 UNION ALL
 SELECT r.kind,r.value,left(COALESCE(t.title||': ','')||t.narrative,2048)
 FROM requested r JOIN journal_revisions t ON r.kind='journal_revision' AND t.journal_revision_id::text=r.value JOIN journal_objects o USING(journal_id)
 WHERE t.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 UNION ALL
 SELECT r.kind,r.value,left(d.payload_text,2048)
 FROM requested r JOIN derived_representations d ON r.kind='derived_representation' AND d.derived_representation_id::text=r.value WHERE d.subject_id=$1 AND d.payload_text IS NOT NULL
 UNION ALL
 SELECT r.kind,r.value,'Observation ('||o.source_class||', observed '||to_char(o.observed_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS')||' UTC): '||COALESCE(description.text,'content descriptor unavailable')
 FROM requested r JOIN observation_occurrences o ON r.kind='occurrence' AND o.occurrence_id::text=r.value
 LEFT JOIN LATERAL (
   SELECT left(d.payload_text,1536) AS text FROM derived_representations d
   WHERE d.subject_id=$1 AND d.payload_text IS NOT NULL AND EXISTS (
     SELECT 1 FROM representation_source_regions($1,d.derived_representation_id) roots JOIN source_regions region USING(source_region_id) WHERE region.artifact_id=o.artifact_id
   ) ORDER BY d.created_at DESC,d.derived_representation_id LIMIT 1
 ) description ON true WHERE o.subject_id=$1
)
SELECT DISTINCT kind,value,text FROM catalog ORDER BY kind,value,text
"#).bind(subject.0).bind(kinds).bind(values).fetch_all(self.pool()).await.map_err(db)?;
        let mut result = rows
            .into_iter()
            .map(|row| {
                Ok(QueryDescriptor {
                    reference: parse_reference(
                        &row.try_get::<String, _>("kind").map_err(db)?,
                        &row.try_get::<String, _>("value").map_err(db)?,
                    )?,
                    text: row.try_get("text").map_err(db)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let tags = refs
            .iter()
            .filter_map(|r| {
                if let CognitiveRef::Tag(t) = r {
                    Some(t.0)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let rows = sqlx::query("SELECT requested AS id,r.label,r.description,r.kind_hint FROM unnest($2::uuid[]) requested JOIN tags t ON t.tag_id=canonical_tag($1,requested) JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id WHERE t.subject_id=$1 AND t.status='active'")
            .bind(subject.0).bind(tags).fetch_all(self.pool()).await.map_err(db)?;
        for row in rows {
            result.push(QueryDescriptor {
                reference: CognitiveRef::Tag(TagId(row.try_get("id").map_err(db)?)),
                text: tag_semantic_representation(
                    &row.try_get::<String, _>("label").map_err(db)?,
                    row.try_get::<Option<String>, _>("description")
                        .map_err(db)?
                        .as_deref(),
                    row.try_get::<Option<String>, _>("kind_hint")
                        .map_err(db)?
                        .as_deref(),
                )?
                .text,
            });
        }
        result.sort_by_key(|descriptor| descriptor.reference.to_string());
        Ok(result)
    }
}
