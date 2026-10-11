// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::memory_service_server::MemoryService {
        forward {
            link_revisions(p::LinkRevisionsRequest) -> ();
            form_memory(p::FormMemoryRequest) -> p::Memory;
            revise_memory(p::ReviseMemoryRequest) -> p::Memory;
            create_episode(p::CreateEpisodeRequest) -> p::EpisodeResponse;
            revise_episode(p::ReviseEpisodeRequest) -> p::EpisodeResponse;
            link_episode_revisions(p::LinkEpisodeRevisionsRequest) -> ();
            read_cognition(p::ReadCognitionRequest) -> p::CognitionResult;
            list_cognitions(p::CognitionListRequest) -> p::CognitionPage;
            list_cognition_history(p::CognitionHistoryRequest) -> p::CognitionPage;
            mutate_cognition(p::CognitionMutationRequest) -> p::CognitionMutationResponse;
        }
        custom {}
    }
}
