"""Typed consumers of evidence-backed Social operations."""

from pydantic import TypeAdapter

from ..contracts.cognition import CognitiveRecord
from ..contracts.motivation import (
    AssessMotivation,
    MotivationAssessment,
    MotivationAssessmentsPage,
    MotivationAssessmentsQuery,
    SavedMotivationAssessment,
)
from ..contracts.social import (
    ChangeSocialRelation,
    IdentityHypothesis,
    LinkIdentity,
    SymbolCandidates,
    SymbolSelection,
    UnlinkIdentity,
)
from ..operations import encoded, segment
from .media import MediaClient


class SocialClient(MediaClient):
    async def motivation_assessment(
        self, namespace_id: str,  assessment_id: str
    ) -> SavedMotivationAssessment:
        return SavedMotivationAssessment.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/motivation/assessments/{segment(assessment_id)}",
            )
        )

    async def motivation_assessments(
        self, namespace_id: str,  query: MotivationAssessmentsQuery
    ) -> MotivationAssessmentsPage:
        return MotivationAssessmentsPage.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/motivation/assessments/query",
                body=encoded(query),
            )
        )

    async def propose_identity(
        self, namespace_id: str,  command: IdentityHypothesis
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/social/identities/hypotheses",
                body=encoded(command),
            )
        )

    async def unlink_identity(
        self, namespace_id: str,  command: UnlinkIdentity
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/social/identities/unlink",
                body=encoded(command),
            )
        )

    async def understand_symbol(
        self, namespace_id: str,  command: SymbolSelection
    ) -> SymbolCandidates:
        return SymbolCandidates.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/social/symbols/understand",
                body=encoded(command),
            )
        )

    async def expression_candidates(
        self, namespace_id: str,  command: SymbolSelection
    ) -> SymbolCandidates:
        return SymbolCandidates.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/social/symbols/express",
                body=encoded(command),
            )
        )

    async def assess_motivation(
        self, namespace_id: str,  command: AssessMotivation
    ) -> MotivationAssessment:
        return MotivationAssessment.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/motivation/assess",
                body=encoded(command),
            )
        )

    async def link_identity(
        self, namespace_id: str,  command: LinkIdentity
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/social/identities",
                body=encoded(command),
            )
        )

    async def resolve_identity(
        self, namespace_id: str,  platform: str, account_id: str
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/social/identities/resolve",
                params={"platform": platform, "account_id": account_id},
            )
        )

    async def change_social_relation(
        self, namespace_id: str,  command: ChangeSocialRelation
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/social/relations",
                body=encoded(command),
            )
        )

    async def social_profile(
        self, namespace_id: str,  entity_id: str
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/social/profile/{segment(entity_id)}",
            )
        )
