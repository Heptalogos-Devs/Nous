//! Exact identity and bounded descriptor SQL mechanics for an owner-projected read view.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{AuthorityStore, database_error as db};
use nous_core::*;
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
            return Err(Error::Invalid(
                "historical view belongs to another Subject".into(),
            ));
        }
        if matches!(reference, CognitiveRef::Tag(_)) || view.cognition_for(reference).is_some() {
            let allowed: bool =
                sqlx::query_scalar("SELECT CASE WHEN $2 THEN self_cognition WHEN $3 THEN social_cognition WHEN $4 THEN motivation ELSE memory END FROM subject_capabilities WHERE subject_id=$1")
                    .bind(subject.0)
                    .bind(reference.domain()==CognitiveDomain::SelfCognition)
                    .bind(reference.domain()==CognitiveDomain::SocialCognition)
                    .bind(reference.domain()==CognitiveDomain::Motivation)
                    .fetch_one(self.pool())
                    .await
                    .map_err(db)?;
            if !allowed {
                return Err(Error::NotFound(
                    "historical Memory Authority is currently disabled".into(),
                ));
            }
        }
        if let CognitiveRef::Tag(tag) = reference {
            return view
                .canonical_tag(*tag)
                .map(|tag| (CognitiveRef::Tag(tag), None, false))
                .ok_or_else(|| Error::NotFound("Tag does not exist in historical view".into()));
        }
        if let Some(state) = view.cognition_for(reference) {
            let object = &state.object == reference;
            let bound = if object {
                state.head.clone()
            } else {
                reference.clone()
            };
            let (kind, value) = reference_parts(&state.object);
            let (table, column) = match kind.as_str() {
                "social_relation" | "language_convention" => ("social_objects", "object_id"),
                "desire" | "concern" | "desired_condition" => ("motivation_objects", "object_id"),
                "self_facet" | "narrative_thread" => ("self_objects", "object_id"),
                "memory" => ("memory_objects", "memory_id"),
                "cognitive_schema" => ("cognitive_schemas", "schema_id"),
                "episode" => ("episode_objects", "episode_id"),
                "journal" => ("journal_objects", "journal_id"),
                _ => {
                    return Err(Error::Infrastructure(
                        "invalid historical cognition owner".into(),
                    ));
                }
            };
            // Only the fixed owner catalog supplies identifiers; Subject and identity are bound.
            let statement =
                format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE subject_id=$1 AND {column}=$2)");
            let available: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(statement.as_str()))
                .bind(subject.0)
                .bind(value.parse::<uuid::Uuid>().map_err(|_| {
                    Error::Infrastructure("invalid historical cognition identity".into())
                })?)
                .fetch_one(self.pool())
                .await
                .map_err(db)?;
            if !available {
                return Err(Error::NotFound(
                    "historical cognition is currently purged or absent".into(),
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
            return Err(Error::NotFound(
                "reference does not exist in historical view".into(),
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
            return Err(Error::Invalid("invalid name locator".into()));
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
                Err(Error::NotFound(_)) => continue,
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
                return Err(Error::Invalid(
                    "identity ambiguity exceeds 64 candidates".into(),
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
}
