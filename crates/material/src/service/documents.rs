//! Effective Material document selection; precise reads can still address older known revisions.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_persistence::database_error as db;
use std::collections::HashSet;

impl MaterialService {
    pub async fn query_descriptors(
        &self,
        subject: SubjectId,
        refs: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
        source_read: Option<&SourceReadView>,
    ) -> Result<Vec<QueryDescriptor>> {
        if refs.len() > 256 || view.is_some_and(|v| v.subject != subject) {
            return Err(Error::Invalid("Material descriptor scope exceeded".into()));
        }
        let (kinds, values): (Vec<_>, Vec<_>) = refs.iter().map(reference_parts).unzip();
        let rows = sqlx::query(r#"
WITH requested AS (SELECT * FROM unnest($2::text[],$3::text[]) r(kind,value) WHERE $5::text[] IS NULL OR source_read_allowed($1,kind,value,$5,$6,$7,$8)), catalog AS (
 SELECT r.kind,r.value,left(d.payload_text,2048) text FROM requested r JOIN derived_representations d ON r.kind='derived_representation' AND d.derived_representation_id::text=r.value WHERE d.subject_id=$1 AND ($4::timestamptz IS NULL OR d.created_at<=$4) AND d.payload_text IS NOT NULL
 UNION ALL
 SELECT r.kind,r.value,'Observation ('||o.source_class||', observed '||to_char(o.observed_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS')||' UTC)'||CASE WHEN $4::timestamptz IS NULL THEN ': '||COALESCE(description.text,'content descriptor unavailable') ELSE '' END
 FROM requested r JOIN observation_occurrences o ON r.kind='occurrence' AND o.occurrence_id::text=r.value
 LEFT JOIN LATERAL (SELECT left(d.payload_text,1536) text FROM derived_representations d WHERE $4::timestamptz IS NULL AND d.subject_id=$1 AND d.payload_text IS NOT NULL AND ($5::text[] IS NULL OR source_read_allowed($1,'derived_representation',d.derived_representation_id::text,$5,$6,$7,$8)) AND EXISTS(SELECT 1 FROM representation_source_regions($1,d.derived_representation_id) roots JOIN source_regions region USING(source_region_id) WHERE region.artifact_id=o.artifact_id) ORDER BY d.created_at DESC,d.derived_representation_id LIMIT 1) description ON true
 WHERE o.subject_id=$1 AND ($4::timestamptz IS NULL OR o.created_at<=$4)
) SELECT DISTINCT kind,value,text FROM catalog ORDER BY kind,value
"#).bind(subject.0).bind(kinds).bind(values).bind(view.map(|v| v.as_of)).bind(source_read.map(|v|v.root_keys.clone())).bind(source_read.map(|v|v.work_contexts.clone()).unwrap_or_default()).bind(source_read.map(|v|v.origin_conversation_refs.clone()).unwrap_or_default()).bind(source_read.map(|v|serde_json::json!(&v.explicit_refs)).unwrap_or_else(||serde_json::json!([]))).fetch_all(self.store.pool()).await.map_err(db)?;
        rows.into_iter()
            .map(|row| {
                Ok(QueryDescriptor {
                    reference: parse_reference(
                        &row.try_get::<String, _>("kind").map_err(db)?,
                        &row.try_get::<String, _>("value").map_err(db)?,
                    )?,
                    text: row.try_get("text").map_err(db)?,
                })
            })
            .collect()
    }

    pub async fn document_text_views(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<std::collections::HashMap<CognitiveRef, MaterialTextView>> {
        if references.len() > 256 {
            return Err(Error::Invalid(
                "Material text lookup supports at most 256 references".into(),
            ));
        }
        let mut tx = self.store.read_view().await?;
        let selected = self
            .document_selection_in(&mut tx, subject, references, view)
            .await?;
        let references: Vec<_> = references
            .iter()
            .filter(|reference| selected.contains(*reference))
            .cloned()
            .collect();
        let inputs = self
            .read_inputs_in(&mut tx, subject, &references, view, true)
            .await?;
        tx.commit().await.map_err(db)?;
        self.read_text_inputs(inputs, self.max_upload_bytes).await
    }

    async fn document_selection_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashSet<CognitiveRef>> {
        if let Some(view) = view {
            if view.subject != subject {
                return Err(Error::Invalid(
                    "Material document view subject mismatch".into(),
                ));
            }
            return Ok(references
                .iter()
                .filter(|reference| view.material_document_contains(reference))
                .cloned()
                .collect());
        }
        let representations: Vec<_> = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::DerivedRepresentation(id) => Some(id.0),
                _ => None,
            })
            .collect();
        let regions: Vec<_> = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::DerivedRegion(id) => Some(id.0),
                _ => None,
            })
            .collect();
        let rows = sqlx::query(r#"
SELECT 'derived_representation' kind,d.derived_representation_id id FROM derived_representations d
 WHERE d.subject_id=$1 AND d.derived_representation_id=ANY($2::uuid[])
 AND NOT EXISTS(SELECT 1 FROM derived_representations newer WHERE newer.subject_id=d.subject_id AND newer.supersedes=d.derived_representation_id)
UNION ALL SELECT 'derived_region',r.derived_region_id FROM derived_regions r JOIN derived_representations d USING(derived_representation_id)
 WHERE r.subject_id=$1 AND r.derived_region_id=ANY($3::uuid[])
 AND NOT EXISTS(SELECT 1 FROM derived_representations newer WHERE newer.subject_id=d.subject_id AND newer.supersedes=d.derived_representation_id)
"#).bind(subject.0).bind(representations).bind(regions).fetch_all(&mut **tx).await.map_err(db)?;
        let mut selected: HashSet<_> = references
            .iter()
            .filter(|reference| {
                matches!(
                    reference,
                    CognitiveRef::Occurrence(_)
                        | CognitiveRef::SourceRegion(_)
                        | CognitiveRef::Artifact(_)
                )
            })
            .cloned()
            .collect();
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(db)?;
            selected.insert(
                match row.try_get::<String, _>("kind").map_err(db)?.as_str() {
                    "derived_representation" => {
                        CognitiveRef::DerivedRepresentation(DerivedRepresentationId(id))
                    }
                    "derived_region" => CognitiveRef::DerivedRegion(DerivedRegionId(id)),
                    _ => {
                        return Err(Error::Infrastructure(
                            "unexpected Material selection kind".into(),
                        ));
                    }
                },
            );
        }
        Ok(selected)
    }
}

/// Identity of the selected UTF8 bytes, shared by exact views and model material.
pub fn text_content_identity(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}
