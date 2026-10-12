// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{ResourceRef, Result, SubjectId};
use nous_runtime::{ResourceDescriptor, ResourceUpsert};

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
            .ok_or_else(|| Error::NotFound("resource not found".into()))
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
            return Err(Error::Invalid("page_size exceeds 200".into()));
        }
        let limit = if page.page_size == 0 {
            50
        } else {
            page.page_size as usize
        };
        let last = if page.page_token.is_empty() {
            String::new()
        } else {
            let (digest, last) = page
                .page_token
                .split_once('.')
                .ok_or_else(|| Error::Invalid("invalid page token".into()))?;
            if digest != blake3::hash(scope.as_bytes()).to_hex().as_str() {
                return Err(Error::Invalid("page token scope mismatch".into()));
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
                    .ok_or_else(|| Error::Internal("page continuation has no last item".into()))?
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
        let families = self
            .0
            .serving
            .family_status(subject)
            .await?
            .into_iter()
            .map(|family| p::ProjectionFamilyStatus {
                name: family.name,
                state: enum_name(family.state).to_uppercase(),
                space_signature: family.space_signature,
                desired_authority_seq: family.desired_authority_seq,
                authority_watermark: family.authority_watermark,
                generation_id: family.generation_id.map(|id| id.0.to_string()),
                config_digest: family.config_digest,
                desired_config_digest: family.desired_config_digest,
            })
            .collect();
        Ok(p::ProjectionStatus { families })
    }
}
