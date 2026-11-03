"""Journal factual support respects its cutoff while revision lineage stays readable."""

import asyncio

import pytest
from nous_sdk.contracts import (
    AdoptCognition,
    EpisodeBody,
    EpisodeMember,
    EpistemicStance,
    EvidenceLink,
    ImportDocument,
    JournalBody,
    MemoryBody,
    TimeRange,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError

from nous.cognition.records import CognitiveRecords
from nous.identity.service import IdentityService
from nous.platform.persistence import Authority


def test_cutoff_covers_evidence_and_allows_prior_journal_lineage(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        cognition = CognitiveRecords(authority)
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await identity.create_namespace(ctx, "N", "namespace")
            await identity.create_subject(ctx, "S", "subject", subject_id="s")
            async with authority.write(ctx) as tx:
                source = await cognition.materials.admit_text_in(
                    tx, ctx, ImportDocument(text="An earlier observation")
                )
                memory = await cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=MemoryBody(text="Earlier knowledge"),
                        evidence=(EvidenceLink(observation_id=source.event["id"]),),
                    ),
                )
                episode = await cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=EpisodeBody(
                            title="Earlier episode",
                            track="temporal",
                            experience=TimeRange(),
                            members=(EpisodeMember(reference=memory.reference, ordinal=0),),
                            boundary_explanation="One accepted event",
                        ),
                        evidence=(EvidenceLink(reference=memory.reference),),
                    ),
                )
                cutoff = tx.commit_seq
            async with authority.write(ctx) as tx:
                later = await cognition.materials.admit_text_in(
                    tx, ctx, ImportDocument(text="A later observation")
                )
                later_memory = await cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=MemoryBody(text="Later knowledge"),
                        evidence=(EvidenceLink(observation_id=later.event["id"]),),
                    ),
                )
            body = JournalBody(
                title="Earlier view",
                narrative="Knowledge available at the cutoff",
                period=TimeRange(),
                knowledge_cutoff=cutoff,
                perspective=EpistemicStance(classification="derived"),
                episodes=(episode.reference,),
            )
            for evidence in (
                EvidenceLink(reference=later_memory.reference),
                EvidenceLink(observation_id=later.event["id"]),
            ):
                with pytest.raises(DomainError, match="writing perspective"):
                    await cognition.adopt(ctx, AdoptCognition(body=body, evidence=(evidence,)))
            journal = await cognition.adopt(
                ctx, AdoptCognition(body=body, evidence=(EvidenceLink(reference=memory.reference),))
            )
            edited = await cognition.adopt(
                ctx,
                AdoptCognition(
                    id=journal.reference.id,
                    expected_revision=journal.reference.revision,
                    body=body.model_copy(
                        update={
                            "narrative": "Edited wording of the earlier view",
                            "lineage": (journal.reference,),
                        }
                    ),
                    evidence=(
                        EvidenceLink(reference=memory.reference),
                        EvidenceLink(reference=journal.reference, polarity="boundary"),
                    ),
                ),
            )
            assert (await cognition.get(ctx, journal.reference)).body.narrative == body.narrative
            assert edited.body.knowledge_cutoff == cutoff
            async with authority.write(ctx) as tx:
                trashed = await cognition.set_deleted_in(tx, ctx, edited.reference, True)
            async with authority.write(ctx) as tx:
                restored = await cognition.set_deleted_in(tx, ctx, trashed.reference, False)
            assert not restored.deleted and restored.body.knowledge_cutoff == cutoff
            with pytest.raises(DomainError, match="recorded history"):
                await cognition.adopt(
                    ctx,
                    AdoptCognition(
                        body=body.model_copy(update={"knowledge_cutoff": 10**9}),
                        evidence=(EvidenceLink(reference=memory.reference),),
                    ),
                )
        finally:
            await authority.close()

    asyncio.run(scenario())
