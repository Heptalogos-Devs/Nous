//! Exact identity and bounded descriptor SQL mechanics for an owner-projected read view.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{AuthorityStore, QueryDescriptor, database_error as db};
use nous_core::*;
use sqlx::Row;
impl AuthorityStore {
    pub async fn bind_reference_in_view(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<(CognitiveRef, Option<i64>, bool)> {
        let Some(view) = view else {
            return self.bind_exact_reference(subject, reference).await;
        };
        if view.subject != subject {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
            ));
        }
        if matches!(reference, CognitiveRef::Tag(_)) || view.cognition_for(reference).is_some() {
            let allowed: bool =
                sqlx::query_scalar("SELECT memory FROM subject_capabilities WHERE subject_id=$1")
                    .bind(subject.0)
                    .fetch_one(self.pool())
                    .await
                    .map_err(db)?;
            if !allowed {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::NotFound,
                    "Not found",
                ));
            }
        }
        if let CognitiveRef::Tag(tag) = reference {
            return view
                .canonical_tag(*tag)
                .map(|tag| (CognitiveRef::Tag(tag), None, false))
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::NotFound,
                        "Tag does not exist in historical view",
                    )
                });
        }
        if let Some(state) = view.cognition_for(reference) {
            let object = &state.object == reference;
            let bound = if object {
                state.head.clone()
            } else {
                reference.clone()
            };
            let (kind, value) = reference_parts(&state.object);
            if !matches!(
                kind.as_str(),
                "memory" | "cognitive_schema" | "episode" | "journal"
            ) {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InternalFailure,
                    "Historical cognition uses an unsupported owner kind",
                ));
            }
            // Current purge is a hard fence even when the requested revision was
            // visible at the historical as-of instant. Shared headers own this
            // identity check for all four cognition kinds.
            let available: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3 AND purge_state='normal')",
            )
                .bind(subject.0)
                .bind(&kind)
                .bind(value.parse::<uuid::Uuid>().map_err(|_| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InternalFailure,
                        "invalid historical cognition identity",
                    )
                })?)
                .fetch_one(self.pool())
                .await
                .map_err(db)?;
            if !available {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::NotFound,
                    "Not found",
                ));
            }
            self.validate_reference(subject, &bound).await?;
            return Ok((bound, state.state["object_epoch"].as_i64(), object));
        }
        if !matches!(
            reference,
            CognitiveRef::Entity(_) | CognitiveRef::Resource(_) | CognitiveRef::ExternalObject(_)
        ) && !view.material_visibility.contains(reference)
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::NotFound,
                "Not found",
            ));
        }
        self.validate_reference(subject, reference).await?;
        Ok((reference.clone(), None, false))
    }
    pub async fn resolve_identity_in_view(
        &self,
        view: &HistoricalAuthoritySnapshot,
        kind: &str,
        locator: &str,
        lexical: bool,
    ) -> Result<(String, Vec<crate::IdentityBinding>)> {
        self.require_subject(view.subject).await?;
        if lexical {
            let prefix = crate::validate_lexical(locator)?;
            if !kind.is_empty() && crate::lexical_prefix(kind)? != prefix {
                return Ok(("REFERENCE_TYPE_MISMATCH".into(), vec![]));
            }
        } else if locator.is_empty() || locator.len() > 256 {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "invalid name locator",
            ));
        }
        let mut bindings = Vec::new();
        for row in &view.lexical_visibility {
            if (!kind.is_empty() && row.object_kind != kind)
                || !(if lexical {
                    row.lexical_ref == locator
                } else {
                    row.display_name == locator || row.aliases.iter().any(|alias| alias == locator)
                })
            {
                continue;
            }
            let requested = parse_reference(&row.object_kind, &row.canonical_ref)?;
            let canonical = if let CognitiveRef::Tag(tag) = requested {
                let Some(tag) = view.canonical_tag(tag) else {
                    continue;
                };
                CognitiveRef::Tag(tag)
            } else {
                requested
            };
            match self
                .bind_reference_in_view(view.subject, &canonical, Some(view))
                .await
            {
                Ok(_) => {}
                Err(Error {
                    code: nous_core::ErrorCode::NotFound,
                    ..
                }) => continue,
                Err(error) => return Err(error),
            };
            if bindings
                .iter()
                .any(|binding: &crate::IdentityBinding| binding.canonical == canonical)
            {
                continue;
            }
            let display_name = if let CognitiveRef::Tag(tag) = canonical {
                view.lexical_visibility
                    .iter()
                    .find(|state| {
                        state.object_kind == "tag" && state.canonical_ref == tag.0.to_string()
                    })
                    .map_or(row.display_name.clone(), |state| state.display_name.clone())
            } else {
                row.display_name.clone()
            };
            bindings.push(crate::IdentityBinding {
                lexical_ref: row.lexical_ref.clone(),
                canonical,
                display_name,
                aliases: row.aliases.clone(),
                status: "LIVE".into(),
            });
            if bindings.len() > 64 {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "Invalid request",
                ));
            }
        }
        Ok((
            match bindings.len() {
                0 => "UNKNOWN_REFERENCE",
                1 => "BOUND",
                _ => "AMBIGUOUS_REFERENCE",
            }
            .into(),
            bindings,
        ))
    }
    pub async fn query_descriptors_in_view(
        &self,
        subject: SubjectId,
        refs: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<QueryDescriptor>> {
        let Some(view) = view else {
            return self.query_descriptors(subject, refs).await;
        };
        if refs.len() > 256 || view.subject != subject {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
            ));
        }
        let mut result = Vec::new();
        let mut selected = Vec::new();
        for reference in refs {
            if let CognitiveRef::Tag(tag) = reference {
                if let Some(canonical) = view.canonical_tag(*tag)
                    && let Some(tag) = view.tags.iter().find(|tag| tag.tag == canonical)
                {
                    result.push(QueryDescriptor {
                        reference: reference.clone(),
                        text: tag.semantic.text.clone(),
                    });
                }
            } else {
                match self
                    .bind_reference_in_view(subject, reference, Some(view))
                    .await
                {
                    Ok((exact, _, _)) => selected.push((reference.clone(), exact)),
                    Err(Error {
                        code: nous_core::ErrorCode::NotFound,
                        ..
                    }) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let (kinds, values): (Vec<_>, Vec<_>) = selected
            .iter()
            .map(|(_, reference)| reference_parts(reference))
            .unzip();
        let rows=sqlx::query(r#"
WITH requested AS (SELECT * FROM unnest($2::text[],$3::text[]) AS r(kind,value)), cognition AS (
 SELECT r.kind,r.value,h.revision_id,h.object_kind
 FROM requested r
 JOIN cognition_revision_headers h ON h.revision_id::text=r.value AND (
     (r.kind='memory_revision' AND h.object_kind='memory')
  OR (r.kind='cognitive_schema_revision' AND h.object_kind='cognitive_schema')
  OR (r.kind='episode_revision' AND h.object_kind='episode')
  OR (r.kind='journal_revision' AND h.object_kind='journal'))
 JOIN cognition_objects o USING(subject_id,object_kind,object_id)
 WHERE h.subject_id=$1 AND h.recorded_at<=$4 AND o.purge_state='normal'
), catalog AS (
 SELECT c.kind,c.value,left(COALESCE(t.title||': ','')||t.representation_text,2048) text FROM cognition c JOIN memory_revisions t ON c.object_kind='memory' AND t.memory_revision_id=c.revision_id
 UNION ALL SELECT c.kind,c.value,left(COALESCE(t.title||': ','')||t.structural_claim||' Scope: '||t.applicability_description||' Boundary: '||t.boundary_definition,2048) FROM cognition c JOIN cognitive_schema_revisions t ON c.object_kind='cognitive_schema' AND t.schema_revision_id=c.revision_id
 UNION ALL SELECT c.kind,c.value,left(COALESCE(t.title||': ','')||t.boundary_explanation,2048) FROM cognition c JOIN episode_revisions t ON c.object_kind='episode' AND t.episode_revision_id=c.revision_id
 UNION ALL SELECT c.kind,c.value,left(COALESCE(t.title||': ','')||t.narrative,2048) FROM cognition c JOIN journal_revisions t ON c.object_kind='journal' AND t.journal_revision_id=c.revision_id
 UNION ALL SELECT r.kind,r.value,left(t.payload_text,2048) FROM requested r JOIN derived_representations t ON r.kind='derived_representation' AND t.derived_representation_id::text=r.value WHERE t.subject_id=$1 AND t.created_at<=$4 AND t.payload_text IS NOT NULL
 UNION ALL SELECT r.kind,r.value,'Observation ('||t.source_class||', observed '||to_char(t.observed_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS')||' UTC)' FROM requested r JOIN observation_occurrences t ON r.kind='occurrence' AND t.occurrence_id::text=r.value WHERE t.subject_id=$1 AND t.created_at<=$4
 UNION ALL SELECT r.kind,r.value,left(v.display_name,2048) FROM requested r JOIN lexical_bindings b ON b.object_kind=r.kind AND b.canonical_ref=r.value JOIN lexical_visibility v USING(lexical_ref) WHERE r.kind IN ('entity','resource','external_object') AND v.subject_id=$1 AND b.tombstoned_at IS NULL
) SELECT kind,value,text FROM catalog ORDER BY kind,value
"#).bind(subject.0).bind(kinds).bind(values).bind(view.as_of).fetch_all(self.pool()).await.map_err(db)?;
        for row in rows {
            let exact = parse_reference(
                &row.try_get::<String, _>("kind").map_err(db)?,
                &row.try_get::<String, _>("value").map_err(db)?,
            )?;
            for (requested, _) in selected.iter().filter(|(_, reference)| *reference == exact) {
                result.push(QueryDescriptor {
                    reference: requested.clone(),
                    text: row.try_get("text").map_err(db)?,
                });
            }
        }
        result.sort_by_key(|descriptor| descriptor.reference.to_string());
        Ok(result)
    }
}
