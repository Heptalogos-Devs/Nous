// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{Result, SubjectId};
fn binding(b: nous_persistence::IdentityBinding) -> p::IdentityBinding {
    p::IdentityBinding {
        lexical_ref: b.lexical_ref,
        canonical: Some(to_ref(b.canonical)),
        display_name: b.display_name,
        aliases: b.aliases,
        status: b.status,
    }
}
impl KernelService {
    pub(super) async fn get_identity_addresses(
        &self,
        input: p::GetIdentityAddressesRequest,
    ) -> Result<p::GetIdentityAddressesResponse> {
        let targets = input
            .targets
            .into_iter()
            .map(|target| {
                Ok(nous_persistence::IdentityAddressTarget {
                    subject: SubjectId(id(&target.subject_id)?),
                    reference: from_ref(required(target.canonical, "canonical")?)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let addresses = self.0.store.identity_addresses(&targets).await?;
        Ok(p::GetIdentityAddressesResponse {
            addresses: addresses
                .into_iter()
                .map(|address| p::IdentityAddress {
                    target: Some(p::IdentityAddressTarget {
                        subject_id: address.target.subject.0.to_string(),
                        canonical: Some(to_ref(address.target.reference)),
                    }),
                    lexical_ref: address.lexical_ref,
                    status: address.status,
                })
                .collect(),
        })
    }
    pub(super) async fn rebind_entity(&self, input: p::RebindEntityRequest) -> Result<()> {
        self.0
            .material
            .rebind_entity(
                SubjectId(id(&input.subject_id)?),
                id(&input.mention_id)?,
                input
                    .entity_ref
                    .map(nous_core::EntityRef::new)
                    .transpose()?,
                input.binding_state,
                input.host_resolution_ref,
                input.reason,
            )
            .await
    }

    pub(super) async fn bind_identity(
        &self,
        input: p::BindIdentityRequest,
    ) -> Result<p::IdentityBinding> {
        let subject = SubjectId(id(&input.subject_id)?);
        let reference = from_ref(required(input.canonical, "canonical")?)?;
        let result = if input.address_only {
            self.0
                .store
                .ensure_identity_address(subject, reference, input.display_name)
                .await?
        } else {
            self.0
                .store
                .bind_identity(subject, reference, input.display_name, input.aliases)
                .await?
        };
        Ok(binding(result))
    }
    pub(super) async fn resolve_identity(
        &self,
        input: p::ResolveIdentityRequest,
    ) -> Result<p::ResolveIdentityResponse> {
        let (locator, lexical) = match required(input.locator, "locator")? {
            p::resolve_identity_request::Locator::Name(name) => (name, false),
            p::resolve_identity_request::Locator::LexicalRef(reference) => (reference, true),
        };
        let subject = if input.subject_id.is_empty() && input.kind == "subject" && lexical {
            self.0.store.subject_for_lexical(&locator).await?
        } else {
            SubjectId(id(&input.subject_id)?)
        };
        let (status, bindings) = if let Some(at) = input.as_of {
            let view = self
                .0
                .historical_authority_view(
                    subject,
                    required(time(Some(at))?, "as_of")?,
                    nous_core::RevisionView::Current,
                )
                .await?;
            self.0
                .store
                .resolve_identity_in_view(&view, &input.kind, &locator, lexical)
                .await?
        } else {
            self.0
                .store
                .resolve_identity(subject, &input.kind, &locator, lexical)
                .await?
        };
        Ok(p::ResolveIdentityResponse {
            status,
            candidates: bindings.into_iter().map(binding).collect(),
        })
    }
}
