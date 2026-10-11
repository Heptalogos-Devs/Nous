// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

fn validate_record(
    record: &ExternalResourceRecord,
    action: &ResourceActionSuggestion,
) -> Result<()> {
    let reference = &record.reference;
    let digest = Sha256::digest(record.content.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if record.resource_ref != action.resource
        || reference.resource_ref != action.resource
        || reference.provider_kind != action.adapter_kind
        || reference.provider_profile != action.provider_profile
        || reference.access_scope != action.provider_locator
        || reference.content_digest != digest
        || reference.profile_digest.len() != 64
        || !reference
            .profile_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || reference.entry_id.is_empty()
        || reference.entry_id.len() > 512
        || reference.provider_resource_id.is_empty()
        || reference.provider_resource_id.len() > 512
        || reference.source_locator.len() > 4096
        || reference
            .entry_version
            .as_ref()
            .is_some_and(|value| value.len() > 512)
        || chrono::DateTime::parse_from_rfc3339(&reference.retrieved_at).is_err()
        || record
            .title
            .as_ref()
            .is_some_and(|value| value.len() > 1024)
        || record.content.len() > 1048576
        || record.provider_rank == 0
        || record
            .provider_score
            .is_some_and(|value| !value.is_finite())
        || !matches!(
            record.access_status.as_str(),
            "allowed" | "denied" | "unknown"
        )
        || !matches!(
            record.version_status.as_str(),
            "current" | "stale" | "missing" | "unknown"
        )
        || (action.current_authority
            && (record.access_status != "allowed" || record.version_status != "current"))
    {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "external resource record violates bound action",
        ));
    }
    Ok(())
}

fn validate_results(
    actions: &HashMap<uuid::Uuid, &ResourceActionSuggestion>,
    incoming: &[ExternalResourceResult],
) -> Result<()> {
    let mut seen = HashSet::new();
    let mut bytes = 0usize;
    for response in incoming {
        let action = actions.get(&response.action_id).ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "unknown resource action identity",
            )
        })?;
        if !seen.insert(response.action_id)
            || response.resource_ref != action.resource
            || response.records.len() > action.limit
            || !matches!(
                response.status.as_str(),
                "success" | "unavailable" | "denied" | "stale" | "failed"
            )
            || (response.status != "success" && !response.records.is_empty())
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "invalid or repeated resource action result",
            ));
        }
        let mut entries = HashSet::new();
        for record in &response.records {
            validate_record(record, action)?;

            if !entries.insert((
                &record.reference.provider_resource_id,
                &record.reference.entry_id,
                &record.reference.content_digest,
            )) {
                return Err(Error::new(
                    nous_core::ErrorOwner::Runtime,
                    nous_core::ErrorCode::InvalidRequest,
                    "duplicate external resource record",
                ));
            }
            bytes = bytes.saturating_add(
                serde_json::to_vec(record)
                    .map_err(|error| {
                        Error::new(
                            nous_core::ErrorOwner::Runtime,
                            nous_core::ErrorCode::InvalidRequest,
                            "Continuation context could not be encoded",
                        )
                        .with_source(error)
                    })?
                    .len(),
            );
            if bytes > 2 * 1024 * 1024 {
                return Err(Error::new(
                    nous_core::ErrorOwner::Runtime,
                    nous_core::ErrorCode::InvalidRequest,
                    "external query records exceed 2 MiB",
                ));
            }
        }
    }
    Ok(())
}

impl CognitiveRuntimeService {
    pub(crate) async fn finalize_resources(
        &self,
        subject: SubjectId,
        result: &mut CognitiveQueryResult,
        mut incoming: Vec<ExternalResourceResult>,
    ) -> Result<()> {
        let actions: HashMap<_, _> = result
            .resource_actions
            .iter()
            .map(|action| (action.action_id, action))
            .collect();
        validate_results(&actions, &incoming)?;
        let current = self
            .list_resources(subject)
            .await?
            .into_iter()
            .map(|resource| {
                (
                    resource.descriptor.resource_ref.clone(),
                    resource.descriptor,
                )
            })
            .collect::<HashMap<_, _>>();
        let mut completed = HashSet::new();
        let order = result
            .resource_actions
            .iter()
            .enumerate()
            .map(|(index, action)| (action.action_id, index))
            .collect::<HashMap<_, _>>();
        incoming.sort_by_key(|response| order[&response.action_id]);
        for mut response in incoming {
            let action = actions[&response.action_id];
            let unchanged = current
                .get(&action.resource)
                .map(descriptor_digest)
                .transpose()?
                .is_some_and(|digest| digest == action.descriptor_digest);

            if !unchanged {
                result.status = QueryStatus::Partial;
                result.degradation.push(Degradation {
                    code: "resource_descriptor_changed_during_query".into(),
                    detail: Some(action.resource.as_str().into()),
                });
                continue;
            }
            if response.status != "success" {
                result.status = if action.current_authority {
                    QueryStatus::Partial
                } else if result.status == QueryStatus::Complete {
                    QueryStatus::Degraded
                } else {
                    result.status
                };
                result.degradation.push(Degradation {
                    code: format!("external_resource_{}", response.status),
                    detail: Some(action.resource.as_str().into()),
                });
                continue;
            }
            response.records.retain(|record| {
                record.access_status != "denied"
                    && !matches!(record.version_status.as_str(), "stale" | "missing")
            });
            response.records.sort_by_key(|record| record.provider_rank);
            result.resource_records.extend(response.records);
            completed.insert(response.action_id);
        }
        result
            .resource_actions
            .retain(|action| !completed.contains(&action.action_id));
        if result.resource_actions.is_empty() {
            result.degradation.retain(|item| {
                !matches!(
                    item.code.as_str(),
                    "required_external_authority_unresolved" | "external_resource_action_required"
                )
            });
            if result.status == QueryStatus::Degraded && result.degradation.is_empty() {
                result.status = QueryStatus::Complete;
            }
        }
        Ok(())
    }
}
