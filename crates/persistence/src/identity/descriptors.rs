//! Identity Directory display metadata, without cognition or Material interpretation.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{AuthorityStore, database_error as db};
use nous_core::*;
use sqlx::Row;
impl AuthorityStore {
    pub async fn identity_descriptors(
        &self,
        subject: SubjectId,
        refs: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<QueryDescriptor>> {
        if refs.len() > 256 || view.is_some_and(|v| v.subject != subject) {
            return Err(Error::Invalid("identity descriptor scope exceeded".into()));
        }
        if let Some(view) = view {
            return Ok(view
                .lexical_visibility
                .iter()
                .filter_map(|entry| {
                    let reference =
                        parse_reference(&entry.object_kind, &entry.canonical_ref).ok()?;
                    refs.contains(&reference).then(|| QueryDescriptor {
                        reference,
                        text: entry.display_name.clone(),
                    })
                })
                .collect());
        }
        let (kinds, values): (Vec<_>, Vec<_>) = refs.iter().map(reference_parts).unzip();
        let rows = sqlx::query("SELECT r.kind,r.value,left(v.display_name || CASE WHEN cardinality(v.aliases)>0 THEN ' (' || array_to_string(v.aliases,', ') || ')' ELSE '' END,2048) text FROM unnest($2::text[],$3::text[]) r(kind,value) JOIN lexical_bindings b ON b.object_kind=r.kind AND b.canonical_ref=r.value JOIN lexical_visibility v USING(lexical_ref) WHERE r.kind IN ('entity','resource','external_object') AND v.subject_id=$1 AND b.tombstoned_at IS NULL ORDER BY r.kind,r.value")
            .bind(subject.0).bind(kinds).bind(values).fetch_all(self.pool()).await.map_err(db)?;
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
}
