"""Typed consumers of evidence-backed Social operations."""

from pydantic import TypeAdapter

from ..contracts.cognition import CognitiveRecord
from ..contracts.social import ChangeSocialRelation, LinkIdentity
from ..operations import encoded, segment
from .media import MediaClient


class SocialClient(MediaClient):
    async def link_identity(
        self, namespace_id: str, subject_id: str, command: LinkIdentity
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/social/identities",
                body=encoded(command),
            )
        )

    async def resolve_identity(
        self, namespace_id: str, subject_id: str, platform: str, account_id: str
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + "/social/identities/resolve",
                params={"platform": platform, "account_id": account_id},
            )
        )

    async def change_social_relation(
        self, namespace_id: str, subject_id: str, command: ChangeSocialRelation
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/social/relations",
                body=encoded(command),
            )
        )

    async def social_profile(
        self, namespace_id: str, subject_id: str, entity_id: str
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/social/profile/{segment(entity_id)}",
            )
        )
