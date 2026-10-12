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
                    display_label: Some(r.display_label),
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
        Ok(resource(result.descriptor))
    }
    pub(super) async fn get_resource(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::ResourceDescriptor> {
        let subject = SubjectId(id(&input.subject_id)?);
        Ok(resource(
            self.0
                .cognition
                .resource(subject, &ResourceRef::new(input.id)?)
                .await?
                .descriptor,
        ))
    }
    pub(super) async fn list_resources(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListResourcesResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let scope = format!("resources:{}:{}", subject.0, input.status);
        let (limit, last) = page_cursor(input.page, &scope)?;
        let (items, more) = self
            .0
            .cognition
            .resource_page(subject, &input.status, limit, last.as_deref())
            .await?;
        let next_page_token = if more {
            next_token(
                &scope,
                items
                    .last()
                    .ok_or_else(|| Error::Internal("page continuation has no last item".into()))?
                    .descriptor
                    .resource_ref
                    .as_str(),
            )
        } else {
            String::new()
        };
        Ok(p::ListResourcesResponse {
            items: items
                .into_iter()
                .map(|view| resource(view.descriptor))
                .collect(),
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
