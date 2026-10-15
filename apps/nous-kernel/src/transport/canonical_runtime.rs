// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::runtime_service_server::RuntimeService {
        forward {
            open_session(p::OpenSessionRequest) -> p::Session;
            get_session(p::ObjectRequest) -> p::Session;
            list_sessions(p::ListRequest) -> p::ListSessionsResponse;
            close_session(p::ObjectRequest) -> p::Session;
            record_observation(p::ObservationInput) -> p::AcceptedObservation;
            report_use(p::ReportUseRequest) -> p::ReportUseResponse;
            create_work_context(p::CreateWorkContextRequest) -> p::WorkContextResponse;
            get_work_context(p::GetWorkContextRequest) -> p::WorkContextResponse;
            list_work_contexts(p::ListWorkContextsRequest) -> p::ListWorkContextsResponse;
            update_work_context(p::UpdateWorkContextRequest) -> p::WorkContextResponse;
            pause_work_context(p::WorkContextMutationRequest) -> p::WorkContextResponse;
            resume_work_context(p::WorkContextMutationRequest) -> p::WorkContextResponse;
            end_work_context(p::WorkContextMutationRequest) -> p::WorkContextResponse;
            set_active_work_context(p::SetActiveWorkContextRequest) -> p::Session;
        }
        custom {}
    }
}
