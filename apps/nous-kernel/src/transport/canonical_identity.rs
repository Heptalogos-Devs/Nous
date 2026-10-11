// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::identity_service_server::IdentityService {
        forward {
            rebind_entity(p::RebindEntityRequest) -> ();
            bind_identity(p::BindIdentityRequest) -> p::IdentityBinding;
            resolve_identity(p::ResolveIdentityRequest) -> p::ResolveIdentityResponse;
            get_identity_addresses(p::GetIdentityAddressesRequest) -> p::GetIdentityAddressesResponse;
        }
        custom {}
    }
}
