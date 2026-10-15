// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::memory_service_server::MemoryService {
        forward {
            get_journal(p::ObjectRequest) -> p::JournalResponse;
            get_journal_revision(p::ObjectRequest) -> p::JournalRevision;
            list_journals(p::ListRequest) -> p::ListJournalsResponse;
            list_journal_revisions(p::ListJournalRevisionsRequest) -> p::ListJournalRevisionsResponse;
            suppress_journal(p::JournalMutationRequest) -> p::JournalResponse;
            restore_journal(p::JournalMutationRequest) -> p::JournalResponse;
            withdraw_journal(p::JournalMutationRequest) -> p::JournalResponse;
            reaccept_journal(p::JournalMutationRequest) -> p::JournalResponse;
            purge_journal(p::JournalMutationRequest) -> ();
            set_accessibility(p::SetAccessibilityRequest) -> p::Memory;
            link_revisions(p::LinkRevisionsRequest) -> ();
            get_memory(p::ObjectRequest) -> p::Memory;
            list_memories(p::ListRequest) -> p::ListMemoriesResponse;
            get_memory_revision(p::ObjectRequest) -> p::Memory;
            list_memory_revisions(p::MemoryHistoryRequest) -> p::ListMemoriesResponse;
            form_memory(p::FormMemoryRequest) -> p::Memory;
            revise_memory(p::ReviseMemoryRequest) -> p::Memory;
            suppress_memory(p::MemoryMutationRequest) -> p::Memory;
            restore_memory(p::MemoryMutationRequest) -> p::Memory;
            withdraw_memory(p::MemoryMutationRequest) -> p::Memory;
            reaccept_memory(p::MemoryMutationRequest) -> p::Memory;
            purge_memory(p::MemoryMutationRequest) -> ();
            create_episode(p::CreateEpisodeRequest) -> p::EpisodeResponse;
            get_episode(p::ObjectRequest) -> p::EpisodeResponse;
            get_episode_revision(p::ObjectRequest) -> p::EpisodeRevision;
            list_episodes(p::ListRequest) -> p::ListEpisodesResponse;
            list_episode_revisions(p::ListEpisodeRevisionsRequest) -> p::ListEpisodeRevisionsResponse;
            revise_episode(p::ReviseEpisodeRequest) -> p::EpisodeResponse;
            link_episode_revisions(p::LinkEpisodeRevisionsRequest) -> ();
            suppress_episode(p::EpisodeMutationRequest) -> p::EpisodeResponse;
            restore_episode(p::EpisodeMutationRequest) -> p::EpisodeResponse;
            withdraw_episode(p::EpisodeMutationRequest) -> p::EpisodeResponse;
            reaccept_episode(p::EpisodeMutationRequest) -> p::EpisodeResponse;
            purge_episode(p::EpisodeMutationRequest) -> ();
        }
        custom {}
    }
}
