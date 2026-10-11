// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod continuation;
mod types;
use crate::*;
use nous_persistence::{ProjectionInvalidation, database_error as db};
use sqlx::Row;
pub use types::*;

impl CognitiveRuntimeService {
    pub async fn upsert_resource(
        &self,
        subject: SubjectId,
        input: ResourceUpsert,
    ) -> Result<ResourceView> {
        self.require_subject(subject).await?;
        let binding = [
            &input.adapter_kind,
            &input.provider_profile,
            &input.provider_locator,
        ];
        if binding.iter().any(|value| !value.is_empty())
            && binding.iter().any(|value| value.is_empty())
        {
            return Err(Error::Invalid(
                "resource provider binding must be complete".into(),
            ));
        }
        if input.adapter_kind.len() > 128
            || input.provider_profile.len() > 128
            || input.provider_locator.len() > 4096
        {
            return Err(Error::Invalid(
                "resource provider binding exceeds bounds".into(),
            ));
        }
        ResourceRef::new(input.resource_ref.as_str())?;
        if input.authority_class.trim().is_empty() {
            return Err(Error::Invalid(
                "resource authority_class is required".into(),
            ));
        }
        if !matches!(
            input.readiness.as_str(),
            "ready" | "degraded" | "unavailable"
        ) {
            return Err(Error::Invalid("invalid resource readiness".into()));
        }
        let mut tx = self.store.begin().await?;
        sqlx::query("INSERT INTO resources(subject_id,resource_ref,display_label,authority_class,coverage,query_dimensions,modalities,freshness_policy,access_cost_class,readiness,updated_at,adapter_kind,provider_profile,provider_locator) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) ON CONFLICT(subject_id,resource_ref) DO UPDATE SET display_label=excluded.display_label,authority_class=excluded.authority_class,coverage=excluded.coverage,query_dimensions=excluded.query_dimensions,modalities=excluded.modalities,freshness_policy=excluded.freshness_policy,access_cost_class=excluded.access_cost_class,readiness=excluded.readiness,updated_at=excluded.updated_at,adapter_kind=excluded.adapter_kind,provider_profile=excluded.provider_profile,provider_locator=excluded.provider_locator")
            .bind(subject.0).bind(input.resource_ref.as_str()).bind(input.display_label).bind(input.authority_class).bind(input.coverage).bind(input.query_dimensions).bind(input.modalities).bind(input.freshness_policy).bind(input.access_cost_class).bind(input.readiness).bind(self.now(subject)).bind(input.adapter_kind).bind(input.provider_profile).bind(input.provider_locator).execute(&mut *tx).await.map_err(db)?;
        let row=sqlx::query("SELECT resource_ref,display_label,authority_class,coverage,query_dimensions,modalities,freshness_policy,access_cost_class,readiness,updated_at,adapter_kind,provider_profile,provider_locator FROM resources WHERE subject_id=$1 AND resource_ref=$2").bind(subject.0).bind(input.resource_ref.as_str()).fetch_one(&mut *tx).await.map_err(db)?;
        let descriptor = decode_resource(subject, row)?;
        self.store
            .ensure_identity_addresses_in(
                &mut tx,
                subject,
                &[CognitiveRef::Resource(input.resource_ref.clone())],
                descriptor.display_label.as_deref().unwrap_or_default(),
            )
            .await?;
        nous_persistence::AuthorityStore::invalidate_in(
            &mut tx,
            subject,
            ProjectionInvalidation {
                topology: true,
                ..ProjectionInvalidation::default()
            },
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(ResourceView { descriptor })
    }

    pub async fn delete_resource(&self, subject: SubjectId, resource: ResourceRef) -> Result<()> {
        let mut tx = self.store.begin().await?;
        sqlx::query("DELETE FROM resources WHERE subject_id=$1 AND resource_ref=$2")
            .bind(subject.0)
            .bind(resource.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("DELETE FROM resident_refs r USING cognitive_sessions s WHERE r.session_id=s.session_id AND s.subject_id=$1 AND r.ref_kind='resource' AND r.ref_value=$2")
            .bind(subject.0)
            .bind(resource.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        nous_persistence::AuthorityStore::invalidate_in(
            &mut tx,
            subject,
            ProjectionInvalidation {
                topology: true,
                ..ProjectionInvalidation::default()
            },
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    pub async fn list_resources(&self, subject: SubjectId) -> Result<Vec<ResourceView>> {
        let rows=sqlx::query("SELECT resource_ref,display_label,authority_class,coverage,query_dimensions,modalities,freshness_policy,access_cost_class,readiness,updated_at,adapter_kind,provider_profile,provider_locator FROM resources WHERE subject_id=$1 ORDER BY resource_ref").bind(subject.0).fetch_all(self.store.pool()).await.map_err(db)?;
        rows.into_iter()
            .map(|row| {
                Ok(ResourceView {
                    descriptor: decode_resource(subject, row)?,
                })
            })
            .collect()
    }

    pub async fn resource_actions_for_query(
        &self,
        query: &CognitiveQuery,
        plan: &QueryPlan,
    ) -> Result<(Vec<ResourceActionSuggestion>, Vec<Degradation>)> {
        if !query.requests_resources() {
            return Ok((Vec::new(), Vec::new()));
        }
        let requested = query
            .expression
            .cues
            .iter()
            .filter_map(|cue| match cue {
                Cue::Resource(r) => Some(r.resource.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let resources = self.list_resources(query.subject).await?;
        let query_text = query
            .expression
            .cues
            .iter()
            .filter_map(|cue| match cue {
                Cue::Text(value) => Some(value.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        let actions = resources
            .into_iter()
            .filter(|r| {
                requested.is_empty() || requested.contains(r.descriptor.resource_ref.as_str())
            })
            .take(plan.resource_limit)
            .map(|r| {
                Ok(ResourceActionSuggestion {
                    action_id: uuid::Uuid::new_v4(),
                    query_text: query_text.clone(),
                    limit: query.result_need.limit.clamp(1, 64),
                    materialize: query.result_need.need_materialization_handles,
                    descriptor_digest: descriptor_digest(&r.descriptor)?,
                    adapter_kind: r.descriptor.adapter_kind,
                    provider_profile: r.descriptor.provider_profile,
                    provider_locator: r.descriptor.provider_locator,
                    resource: r.descriptor.resource_ref,
                    action: if query.resources.synopsis_only
                        || plan.prefer_resource_synopsis
                        || query_text.is_empty()
                    {
                        "inspect_synopsis"
                    } else if query.expression.constraints.current_authority
                        != CurrentAuthorityNeed::None
                    {
                        "query_current_authority"
                    } else {
                        "search"
                    }
                    .into(),
                    reason: format!(
                        "Consumer must access external authority; descriptor readiness={}",
                        r.descriptor.readiness
                    ),
                    current_authority: matches!(
                        query.expression.constraints.current_authority,
                        CurrentAuthorityNeed::Prefer | CurrentAuthorityNeed::Required
                    ),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let degradation = vec![Degradation {
            code: if query.expression.constraints.current_authority
                == CurrentAuthorityNeed::Required
            {
                "required_external_authority_unresolved"
            } else {
                "external_resource_action_required"
            }
            .into(),
            detail: Some(
                "Kernel returned resource awareness; external access has not occurred".into(),
            ),
        }];
        Ok((actions, degradation))
    }
}

pub(crate) fn descriptor_digest(descriptor: &ResourceDescriptor) -> Result<String> {
    Ok(blake3::hash(
        &serde_json::to_vec(descriptor).map_err(|error| Error::Invalid(error.to_string()))?,
    )
    .to_hex()
    .to_string())
}

pub(crate) fn decode_resource(
    subject: SubjectId,
    row: sqlx::postgres::PgRow,
) -> Result<ResourceDescriptor> {
    Ok(ResourceDescriptor {
        subject_id: subject,
        resource_ref: ResourceRef::new(row.try_get::<String, _>("resource_ref").map_err(db)?)?,
        display_label: row.try_get("display_label").map_err(db)?,
        authority_class: row.try_get("authority_class").map_err(db)?,
        coverage: row.try_get("coverage").map_err(db)?,
        query_dimensions: row.try_get("query_dimensions").map_err(db)?,
        modalities: row.try_get("modalities").map_err(db)?,
        freshness_policy: row.try_get("freshness_policy").map_err(db)?,
        access_cost_class: row.try_get("access_cost_class").map_err(db)?,
        readiness: row.try_get("readiness").map_err(db)?,
        updated_at: row.try_get("updated_at").map_err(db)?,
        adapter_kind: row.try_get("adapter_kind").map_err(db)?,
        provider_profile: row.try_get("provider_profile").map_err(db)?,
        provider_locator: row.try_get("provider_locator").map_err(db)?,
    })
}
