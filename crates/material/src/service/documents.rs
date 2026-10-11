//! Effective Material document selection; precise reads can still address older known revisions.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_persistence::database_error as db;
use std::collections::HashSet;

impl MaterialService {
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
        let selected = self.document_selection(subject, references, view).await?;
        let mut result = std::collections::HashMap::new();
        for reference in references {
            if selected.contains(reference)
                && !result.contains_key(reference)
                && let Some(text) = self.text_view(subject, reference, view).await?
            {
                result.insert(reference.clone(), text);
            }
        }
        Ok(result)
    }

    pub(super) async fn document_selection(
        &self,
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
"#).bind(subject.0).bind(representations).bind(regions).fetch_all(self.store.pool()).await.map_err(db)?;
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
            selected.insert(parse_reference(
                &row.try_get::<String, _>("kind").map_err(db)?,
                &row.try_get::<Uuid, _>("id").map_err(db)?.to_string(),
            )?);
        }
        Ok(selected)
    }
}

/// Identity of the selected UTF8 bytes, shared by exact views and model material.
pub fn text_content_identity(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}
