// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::system_service_server::SystemService {
        forward {
            get_projection_status(p::SubjectRequest) -> p::ProjectionStatus;
            reclaim_serving(p::SubjectRequest) -> p::ServingReclamation;
        }
        custom {
            async fn get_status(
                &self,
                request: Request<()>,
            ) -> std::result::Result<Response<p::SystemStatus>, Status> {
                request.into_inner();
                KernelService::get_status(self, ())
                    .await
                    .map(Response::new)
                    .map_err(status)
            }

        }
    }
}
