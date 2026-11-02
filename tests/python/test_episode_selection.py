"""Episode track and person selection follows stable cognition identities and half-open time."""

import asyncio

from nous_sdk.contracts import (
    AdoptCognition,
    EntityBody,
    EntityMention,
    EpisodeBody,
    EpisodeMember,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    MemoryChange,
    TimeRange,
)
from nous.platform.call_context import CallContext
from nous_sdk.contracts.episodes import EpisodeQuery

from nous.application import Application
from nous.platform.configuration import Settings


def test_person_and_tracks_survive_member_revision_without_touching_time_boundaries(tmp_path):
    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await app.identity.create_namespace(ctx, "N", "namespace")
            await app.identity.create_subject(ctx, "S", "subject", subject_id="s")
            async with app.authority.write(ctx) as tx:
                original = await app.material.admit_text_in(
                    tx, ctx, ImportDocument(text="An accepted encounter")
                )
                memory = await app.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=MemoryBody(text="Original account"),
                        evidence=(EvidenceLink(observation_id=original.event["id"]),),
                    ),
                )
                person = await app.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=EntityBody(
                            label="Participant",
                            mentions=(EntityMention(reference=memory.reference),),
                        ),
                        evidence=(EvidenceLink(reference=memory.reference),),
                    ),
                )
            revised = await app.memories.change_memory(
                ctx,
                MemoryChange(
                    reference=memory.reference,
                    action="revise",
                    body=MemoryBody(text="Corrected account"),
                ),
            )
            episodes = []
            for track in ("temporal", "person/participant", "activity/work"):
                episodes.append(
                    await app.cognition.adopt(
                        ctx,
                        AdoptCognition(
                            body=EpisodeBody(
                                title=track,
                                track=track,
                                experience=TimeRange(start=10, end=20),
                                members=(EpisodeMember(reference=revised.reference, ordinal=0),),
                                boundary_explanation="One accepted member",
                            ),
                            evidence=(EvidenceLink(reference=revised.reference),),
                        ),
                    )
                )
            related = await app.episodes.query(ctx, EpisodeQuery(person=person.reference))
            assert {view.record.reference for view in related} == {
                record.reference for record in episodes
            }
            selected = await app.episodes.query(ctx, EpisodeQuery(track="activity/work"))
            assert len(selected) == 1 and selected[0].record.reference == episodes[2].reference
            assert not await app.episodes.query(
                ctx, EpisodeQuery(experience=TimeRange(start=0, end=10))
            )
            assert not await app.episodes.query(
                ctx, EpisodeQuery(experience=TimeRange(start=20, end=30))
            )
            assert (
                len(
                    await app.episodes.query(
                        ctx, EpisodeQuery(experience=TimeRange(start=19, end=21))
                    )
                )
                == 3
            )
        finally:
            await app.close()

    asyncio.run(scenario())
