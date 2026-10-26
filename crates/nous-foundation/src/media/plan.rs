use super::{Media, MediaPolicy, PreparationRequest};
use crate::platform::ExecutableResource;
use nous_domain::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct PreparationPlan {
    pub(super) request: PreparationRequest,
    pub(super) policy: MediaPolicy,
    resources: BTreeMap<String, Option<ExecutableResource>>,
    policy_digest: String,
    fingerprint: String,
}
impl PreparationPlan {
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }
    pub fn request(&self) -> &PreparationRequest {
        &self.request
    }
}
impl Media {
    pub async fn freeze_preparation(&self, request: PreparationRequest) -> Result<PreparationPlan> {
        request.validate()?;
        let policy = self.policy().await?;
        let resources = [
            Some(policy.ffmpeg_resource.as_str()),
            Some(policy.probe_resource.as_str()),
            policy.pdf_resource.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(|key| (key.to_owned(), self.platform.resource_configuration(key)))
        .collect::<BTreeMap<_, _>>();
        let policy_digest = hash(&(&policy, &resources))?;
        let fingerprint = hash(&(&request, &policy_digest))?;
        Ok(PreparationPlan {
            request,
            policy,
            resources,
            policy_digest,
            fingerprint,
        })
    }
}
fn hash(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
