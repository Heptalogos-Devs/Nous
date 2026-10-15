// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::subject_service_server::SubjectService {
        forward {
            provision_capabilities(p::ProvisionCapabilitiesRequest) -> p::Subject;
            create_subject(p::CreateSubjectRequest) -> p::Subject;
            get_subject(p::SubjectRequest) -> p::Subject;
            list_subjects(p::ListRequest) -> p::ListSubjectsResponse;
            get_cognitive_seed(p::SubjectRequest) -> p::CognitiveSeedVersion;
            adopt_cognitive_seed(p::AdoptCognitiveSeedRequest) -> p::CognitiveSeedVersion;
        }
        custom {}
    }
}
