"""Scoped Episode management shares the public domain operations."""

from pydantic import TypeAdapter

from ..contracts import CognitiveRecord, CognitiveRef
from ..contracts.episodes import (
    EpisodeGet,
    EpisodeQuery,
    EpisodeRebuildSubmission,
    EpisodeView,
    ProcessEpisodeSources,
    RebuildEpisodes,
)
from ..operations import encoded, object_value, segment
from ..types import JsonObject
from .core import ClientCore


class EpisodeClient(ClientCore):
    async def episode_history(
        self, namespace_id: str, subject_id: str, episode_id: str, *, limit: int = 100
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/episodes/{segment(episode_id)}/history",
                params={"limit": limit},
            )
        )

    async def query_episodes(
        self, namespace_id: str, subject_id: str, command: EpisodeQuery
    ) -> tuple[EpisodeView, ...]:
        return TypeAdapter(tuple[EpisodeView, ...]).validate_python(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/episodes/query",
                body=encoded(command),
            )
        )

    async def episode(
        self,
        namespace_id: str,
        subject_id: str,
        reference: CognitiveRef,
        *,
        paragraph_limit: int = 100,
    ) -> EpisodeView:
        return EpisodeView.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/episodes/get",
                body=encoded(EpisodeGet(reference=reference, paragraph_limit=paragraph_limit)),
            )
        )

    async def rebuild_episodes(
        self, namespace_id: str, subject_id: str, command: RebuildEpisodes
    ) -> EpisodeRebuildSubmission:
        return EpisodeRebuildSubmission.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/episodes/rebuild",
                body=encoded(command),
            )
        )

    async def episode_status(
        self, namespace_id: str, subject_id: str, *, limit: int = 20
    ) -> JsonObject:
        return object_value(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + "/episodes/status",
                params={"limit": limit},
            )
        )

    async def process_episode_sources(
        self, namespace_id: str, subject_id: str, command: ProcessEpisodeSources
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/episodes/process",
                body=encoded(command),
            )
        )
