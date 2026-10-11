// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{ResourceRef, Result, SubjectId};
use nous_persistence::database_error as db;
use nous_runtime::{ResourceDescriptor, ResourceUpsert};
use sqlx::Row;

fn json_strings(value: serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn resource(r: ResourceDescriptor) -> p::ResourceDescriptor {
    p::ResourceDescriptor {
        resource_ref: r.resource_ref.as_str().into(),
        display_label: r.display_label.unwrap_or_default(),
        authority_class: r.authority_class,
        coverage: to_object(r.coverage),
        query_dimensions: json_strings(r.query_dimensions),
        modalities: json_strings(r.modalities),
        freshness_policy: to_object(r.freshness_policy),
        access_cost_class: r.access_cost_class,
        readiness: r.readiness,
        adapter_kind: r.adapter_kind,
        provider_profile: r.provider_profile,
        provider_locator: r.provider_locator,
    }
}
impl KernelService {
    pub(super) async fn put_resource(
        &self,
        input: p::PutResourceRequest,
    ) -> Result<p::ResourceDescriptor> {
        let r = required(input.descriptor, "descriptor")?;
        let subject = SubjectId(id(&input.subject_id)?);
        let result = self
            .0
            .cognition
            .upsert_resource(
                subject,
                ResourceUpsert {
                    resource_ref: ResourceRef::new(&r.resource_ref)?,
                    adapter_kind: r.adapter_kind,
                    provider_profile: r.provider_profile,
                    provider_locator: r.provider_locator,
                    display_label: Some(r.display_label.clone()),
                    authority_class: r.authority_class,
                    coverage: object(r.coverage),
                    query_dimensions: serde_json::json!(r.query_dimensions),
                    modalities: serde_json::json!(r.modalities),
                    freshness_policy: object(r.freshness_policy),
                    access_cost_class: r.access_cost_class,
                    readiness: r.readiness,
                },
            )
            .await?;
        self.0
            .store
            .bind_identity(
                subject,
                nous_core::CognitiveRef::Resource(ResourceRef::new(r.resource_ref)?),
                r.display_label,
                vec![],
            )
            .await?;
        Ok(resource(result.descriptor))
    }
    pub(super) async fn get_resource(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::ResourceDescriptor> {
        let subject = SubjectId(id(&input.subject_id)?);
        self.0.store.require_subject(subject).await?;
        self.0
            .cognition
            .list_resources(subject)
            .await?
            .into_iter()
            .find(|r| r.descriptor.resource_ref.as_str() == input.id)
            .map(|r| resource(r.descriptor))
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::NotFound,
                    "resource not found",
                )
            })
    }
    pub(super) async fn list_resources(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListResourcesResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        self.0.store.require_subject(subject).await?;
        let scope = format!("resources:{}:{}", input.subject_id, input.status);
        let page = input.page.unwrap_or_default();
        if page.page_size > 200 {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "page_size exceeds 200",
            ));
        }
        let limit = if page.page_size == 0 {
            50
        } else {
            page.page_size as usize
        };
        let last = if page.page_token.is_empty() {
            String::new()
        } else {
            let (digest, last) = page.page_token.split_once('.').ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "invalid page token",
                )
            })?;
            if digest != blake3::hash(scope.as_bytes()).to_hex().as_str() {
                return Err(Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "page token scope mismatch",
                ));
            }
            last.to_owned()
        };
        let mut items: Vec<_> = self
            .0
            .cognition
            .list_resources(subject)
            .await?
            .into_iter()
            .filter(|r| {
                r.descriptor.resource_ref.as_str() > last.as_str()
                    && (input.status.is_empty() || r.descriptor.readiness == input.status)
            })
            .take(limit + 1)
            .map(|r| resource(r.descriptor))
            .collect();
        let more = items.len() > limit;
        items.truncate(limit);
        let next_page_token = if more {
            format!(
                "{}.{}",
                blake3::hash(scope.as_bytes()).to_hex(),
                items
                    .last()
                    .ok_or_else(|| Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "page continuation has no last item"
                    ))?
                    .resource_ref
            )
        } else {
            String::new()
        };
        Ok(p::ListResourcesResponse {
            items,
            next_page_token,
        })
    }
    pub(super) async fn remove_resource(&self, input: p::ObjectRequest) -> Result<()> {
        self.0
            .cognition
            .delete_resource(
                SubjectId(id(&input.subject_id)?),
                ResourceRef::new(input.id)?,
            )
            .await
    }
    pub(super) async fn get_status(&self, _: ()) -> Result<p::SystemStatus> {
        let status = self.0.status().await;
        Ok(p::SystemStatus {
            components: status
                .capabilities
                .into_iter()
                .map(|c| p::ComponentStatus {
                    name: c.capability_id,
                    state: enum_name(c.status),
                    detail: c.reason,
                })
                .collect(),
        })
    }
    pub(super) async fn get_projection_status(
        &self,
        input: p::SubjectRequest,
    ) -> Result<p::ProjectionStatus> {
        let subject = SubjectId(id(&input.subject_id)?);
        self.0.store.require_subject(subject).await?;
        let snapshot = self.0.configuration.snapshot_for_subject(subject)?;
        let rows=sqlx::query("SELECT COALESCE(w.family,c.family) AS family,COALESCE(w.space_signature,c.space_signature) AS space_signature,COALESCE(w.desired_authority_seq,0) AS desired_authority_seq,g.authority_watermark,g.generation_id,g.config_digest FROM projection_watermarks w FULL JOIN serving_current c ON c.subject_id=w.subject_id AND c.family=w.family AND c.space_signature=w.space_signature LEFT JOIN serving_generations g ON g.generation_id=c.generation_id WHERE COALESCE(w.subject_id,c.subject_id)=$1 ORDER BY family,space_signature")
            .bind(subject.0).fetch_all(self.0.store.pool()).await.map_err(db)?;
        let mut families = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get("family").map_err(db)?;
            let desired_authority_seq = row.try_get("desired_authority_seq").map_err(db)?;
            let authority_watermark: Option<i64> =
                row.try_get("authority_watermark").map_err(db)?;
            let config_digest: Option<String> = row.try_get("config_digest").map_err(db)?;
            let desired_config_digest = self
                .0
                .serving
                .config_digest(subject, &name, &snapshot)
                .await?;
            let state = match authority_watermark {
                Some(watermark)
                    if watermark >= desired_authority_seq
                        && config_digest.as_deref() == Some(desired_config_digest.as_str()) =>
                {
                    "READY"
                }
                Some(_) => "STALE",
                None => "UNAVAILABLE",
            };
            families.push(p::ProjectionFamilyStatus {
                name,
                state: state.into(),
                space_signature: row.try_get("space_signature").map_err(db)?,
                desired_authority_seq,
                authority_watermark,
                generation_id: row
                    .try_get::<Option<uuid::Uuid>, _>("generation_id")
                    .map_err(db)?
                    .map(|id| id.to_string()),
                config_digest,
                desired_config_digest,
            });
        }
        Ok(p::ProjectionStatus { families })
    }
}
