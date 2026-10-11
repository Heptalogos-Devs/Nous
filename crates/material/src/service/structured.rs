// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;
use std::collections::HashSet;

fn requested_basis(payload: &serde_json::Value) -> Result<HashSet<CognitiveRef>> {
    let mut requested = HashSet::new();
    let summary = payload
        .get("summary")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "structured summary missing",
            )
        })?;
    let mut fields = vec![summary];
    for group in [
        "observations",
        "mentions",
        "embedded_text",
        "source_text",
        "speech",
        "interpretations",
        "uncertainties",
    ] {
        let items = payload
            .get(group)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Material,
                    nous_core::ErrorCode::InvalidRequest,
                    "structured interpretation field group missing",
                )
            })?;
        if items.len() > 64 {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "structured interpretation field count exceeded",
            ));
        }
        fields.extend(items);
    }
    for item in fields {
        if item.get("basis_keys").is_some() {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "invocation-local support keys cannot be persisted",
            ));
        }
        let basis_refs = item
            .get("basis_refs")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Material,
                    nous_core::ErrorCode::InvalidRequest,
                    "structured field lacks stable basis_refs",
                )
            })?;
        if basis_refs.len() > 16
            || ((matches!(
                item.get("basis").and_then(serde_json::Value::as_str),
                Some("direct" | "reported")
            ) || (std::ptr::eq(item, summary)
                && summary
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())))
                && basis_refs.is_empty())
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "structured field support bounds invalid",
            ));
        }
        for basis in basis_refs {
            let kind = basis
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Material,
                        nous_core::ErrorCode::InvalidRequest,
                        "invalid support kind",
                    )
                })?;
            let value = basis
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Material,
                        nous_core::ErrorCode::InvalidRequest,
                        "invalid support identity",
                    )
                })?;
            requested.insert(nous_core::parse_reference(kind, value)?);
        }
    }
    Ok(requested)
}

impl MaterialService {
    pub(super) async fn validate_field_basis(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        representation: &DerivedRepresentation,
    ) -> Result<()> {
        let Some(payload) = &representation.payload_json else {
            return Ok(());
        };
        let requested = requested_basis(payload)?;
        let mut allowed: HashSet<_> = representation
            .inputs
            .iter()
            .map(|item| item.reference.clone())
            .collect();
        let representations: Vec<_> = representation
            .inputs
            .iter()
            .filter_map(|item| match item.reference {
                CognitiveRef::DerivedRepresentation(id) => Some(id.0),
                _ => None,
            })
            .collect();
        let regions: Vec<_> = representation
            .inputs
            .iter()
            .filter_map(|item| match item.reference {
                CognitiveRef::DerivedRegion(id) => Some(id.0),
                _ => None,
            })
            .collect();
        let rows = sqlx::query("WITH RECURSIVE ancestry(id) AS (SELECT derived_representation_id FROM derived_representations WHERE subject_id=$1 AND derived_representation_id=ANY($2) UNION SELECT derived_representation_id FROM derived_regions WHERE subject_id=$1 AND derived_region_id=ANY($3) UNION SELECT COALESCE(i.input_representation_id,r.derived_representation_id) FROM ancestry a JOIN derived_representation_inputs i ON i.derived_representation_id=a.id LEFT JOIN derived_regions r ON r.derived_region_id=i.derived_region_id WHERE i.input_representation_id IS NOT NULL OR r.derived_representation_id IS NOT NULL) SELECT 'derived_representation' AS kind,id FROM ancestry UNION SELECT 'derived_region',r.derived_region_id FROM derived_regions r JOIN ancestry a ON a.id=r.derived_representation_id WHERE r.subject_id=$1 UNION SELECT 'source_region',i.source_region_id FROM derived_representation_inputs i JOIN ancestry a ON a.id=i.derived_representation_id WHERE i.source_region_id IS NOT NULL")
            .bind(representation.subject_id.0).bind(representations).bind(regions).fetch_all(&mut **tx).await.map_err(db)?;
        for row in rows {
            let kind: String = row.try_get("kind").map_err(db)?;
            let id: Uuid = row.try_get("id").map_err(db)?;
            allowed.insert(nous_core::parse_reference(&kind, &id.to_string())?);
        }
        if !requested.is_subset(&allowed) {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "structured field support is outside its input graph",
            ));
        }
        Ok(())
    }
}
