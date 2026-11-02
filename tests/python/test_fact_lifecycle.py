"""Fact content ownership and shared object/source lifecycle remain distinct operations."""

import asyncio

import pytest
from nous_sdk.contracts import (
    AdoptCognition,
    RevisionCause,
    SourceCommand,
    TimeRange,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.facts import ChatFactScopeInput, FactChange, FactClaimInput, FactQuery

from nous.application import Application
from nous.cognition.revision_state import RevisionState
from nous.platform.configuration import Settings


def test_fact_source_withdrawal_and_object_state_preserve_claim_content(tmp_path):
    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await app.identity.create_namespace(ctx, "N", "namespace")
            await app.identity.create_subject(ctx, "S", "subject", subject_id="s")
            created = await app.facts.change(
                ctx,
                FactChange(
                    action="assert",
                    refresh_profile=False,
                    claim=FactClaimInput(
                        scope=ChatFactScopeInput(conversation_ref="meeting"),
                        fact_key="day",
                        value_text="Thursday",
                    ),
                ),
            )
            record = created.record
            await app.sources.trash(
                ctx,
                SourceCommand(observation_ids=(created.observation_id,), reason="Withdraw source"),
            )
            assert not await app.facts.query(ctx, FactQuery())
            await app.sources.restore(
                ctx,
                SourceCommand(observation_ids=(created.observation_id,), reason="Restore source"),
            )
            current = (await app.facts.query(ctx, FactQuery()))[0]
            assert current.reference == record.reference and current.body == record.body
            trashed = await app.sources.set_deleted(ctx, record.reference, True, "trash-object")
            assert trashed.deleted and trashed.body == record.body
            assert trashed.times.model_dump(exclude={"recorded"}) == record.times.model_dump(
                exclude={"recorded"}
            )
            restored = await app.sources.set_deleted(
                ctx, trashed.reference, False, "restore-object"
            )
            assert not restored.deleted and restored.body == record.body
            assert restored.producer_call_id == record.producer_call_id
            with pytest.raises(DomainError, match="Fact owner"):
                await app.cognition.adopt(
                    ctx,
                    AdoptCognition(
                        id=restored.reference.id,
                        expected_revision=restored.reference.revision,
                        body=record.body.model_copy(update={"value_text": "Friday"}),
                        evidence=record.evidence,
                    ),
                )
            for body, times in (
                (record.body.model_copy(update={"status": "retracted"}), restored.times),
                (
                    record.body,
                    restored.times.model_copy(update={"validity": TimeRange(start=1, end=2)}),
                ),
            ):
                with pytest.raises(DomainError, match="Fact owner"):
                    async with app.authority.write(ctx) as tx:
                        await app.cognition.adopt_in(
                            tx,
                            ctx,
                            AdoptCognition(
                                id=restored.reference.id,
                                expected_revision=restored.reference.revision,
                                body=body,
                                times=times,
                                evidence=restored.evidence,
                                producer_call_id=restored.producer_call_id,
                            ),
                            state=RevisionState(
                                deleted=True,
                                cause=RevisionCause(
                                    action="fact.forged",
                                    actor=ctx.caller_id,
                                    reason="Unowned content change",
                                ),
                            ),
                        )
        finally:
            await app.close()

    asyncio.run(scenario())
