"""Corrected content follows its real new evidence, with old evidence retained as history."""

import asyncio

import pytest
from nous_sdk.contracts import (
    AdmitFiles,
    CallContext,
    ContactMaterial,
    DomainError,
    ImportDocument,
    MediaReference,
    SourceCommand,
    TimePoint,
)
from nous_sdk.contracts.corrections import (
    ApplyCorrection,
    CorrectionEdit,
    PlanCorrection,
    RollbackCorrection,
)
from nous_sdk.contracts.facts import ChatFactScopeInput, FactChange, FactClaimInput, FactQuery

from nous.application import Application
from nous.platform.configuration import Settings


@pytest.mark.parametrize("actual_support", [False, True])
def test_memory_correction_withdrawal_and_rollback_keep_only_actual_support(
    tmp_path, actual_support
):
    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await app.identity.create_namespace(ctx, "N", "namespace")
            await app.identity.create_subject(ctx, "S", "subject", subject_id="s")
            imported = await app.imports.admit_files(
                ctx,
                AdmitFiles(
                    inputs=(
                        ImportDocument(
                            text="Meeting is Tuesday.", observed_at=TimePoint(coordinate=100)
                        ),
                    ),
                    llm_enabled=False,
                    vectors="disabled",
                ),
            )
            before = await app.cognition.get(ctx, imported.outputs[0].cognition[0])
            support_refs = ()
            if actual_support:
                async with app.authority.write(ctx) as tx:
                    source_b = await app.material.admit_text_in(
                        tx, ctx, ImportDocument(text="The meeting was corrected to Thursday.")
                    )
                contact = await app.imports.contact(
                    ctx,
                    ContactMaterial(
                        source=MediaReference(kind="material", id=source_b.material["id"]),
                        external_event_id="correction-contact",
                        observed_at=TimePoint(coordinate=200),
                    ),
                )
                support_refs = contact.cognition
            plan = await app.corrections.plan(
                ctx,
                PlanCorrection(
                    instruction="Correct Tuesday to Thursday",
                    edits=(
                        CorrectionEdit(
                            reference=before.reference,
                            body=before.body.model_copy(update={"text": "Meeting is Thursday."}),
                        ),
                    ),
                    supports=support_refs,
                ),
            )
            changed = (await app.corrections.apply(ctx, ApplyCorrection(plan_id=plan.id))).records[
                0
            ]
            assert changed.body.text == "Meeting is Thursday." and not changed.body.media
            assert imported.outputs[0].observation_id not in changed.support_roots
            assert any(
                link.reference == before.reference and link.polarity == "boundary"
                for link in changed.evidence
            )
            withdrawn = contact.observation_id if actual_support else plan.source_observation_id
            if actual_support:
                assert plan.source_observation_id not in changed.support_roots
                assert changed.times.observation.coordinate == 200
            await app.sources.trash(
                ctx,
                SourceCommand(observation_ids=(withdrawn,), reason="Withdraw correction source B"),
            )
            with pytest.raises(DomainError, match="sources"):
                await app.cognition.get(ctx, changed.reference)
            assert (
                await app.cognition.get(ctx, before.reference)
            ).body.text == "Meeting is Tuesday."
            await app.sources.restore(
                ctx, SourceCommand(observation_ids=(withdrawn,), reason="Restore source B")
            )
            assert (
                await app.cognition.get(ctx, changed.reference)
            ).body.text == "Meeting is Thursday."
            restored = (
                await app.corrections.rollback(ctx, RollbackCorrection(plan_id=plan.id))
            ).records[0]
            assert restored.body.text == "Meeting is Tuesday."
            await app.sources.trash(
                ctx,
                SourceCommand(
                    observation_ids=(imported.outputs[0].observation_id,),
                    reason="Withdraw original source A",
                ),
            )
            with pytest.raises(DomainError, match="sources"):
                await app.cognition.get(ctx, restored.reference)
        finally:
            await app.close()

    asyncio.run(scenario())


def test_fact_correction_uses_real_support_instead_of_its_instruction_report(tmp_path):
    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await app.identity.create_namespace(ctx, "N", "namespace")
            await app.identity.create_subject(ctx, "S", "subject", subject_id="s")
            original = await app.facts.change(
                ctx,
                FactChange(
                    action="assert",
                    refresh_profile=False,
                    claim=FactClaimInput(
                        scope=ChatFactScopeInput(conversation_ref="meeting"),
                        fact_key="day",
                        value_text="Tuesday",
                        cardinality="single",
                    ),
                ),
            )
            async with app.authority.write(ctx) as tx:
                source = await app.material.admit_text_in(
                    tx, ctx, ImportDocument(text="The day is Thursday.")
                )
            support = await app.imports.contact(
                ctx,
                ContactMaterial(
                    source=MediaReference(kind="material", id=source.material["id"]),
                    external_event_id="fact-correction-contact",
                ),
            )
            plan = await app.corrections.plan(
                ctx,
                PlanCorrection(
                    instruction="Update the day from real source B",
                    supports=support.cognition,
                    edits=(
                        CorrectionEdit(
                            reference=original.record.reference,
                            body=original.record.body.model_copy(update={"value_text": "Thursday"}),
                        ),
                    ),
                ),
            )
            await app.corrections.apply(ctx, ApplyCorrection(plan_id=plan.id))
            changed = (await app.facts.query(ctx, FactQuery()))[0]
            assert (
                changed.body.value_text == "Thursday"
                and plan.source_observation_id not in changed.support_roots
            )
            await app.sources.trash(
                ctx,
                SourceCommand(
                    observation_ids=(support.observation_id,), reason="Withdraw actual support B"
                ),
            )
            assert not await app.facts.query(ctx, FactQuery())
            await app.sources.restore(
                ctx,
                SourceCommand(
                    observation_ids=(support.observation_id,), reason="Restore actual support B"
                ),
            )
            await app.corrections.rollback(ctx, RollbackCorrection(plan_id=plan.id))
            restored = (await app.facts.query(ctx, FactQuery()))[0]
            assert restored.body.value_text == "Tuesday"
            await app.sources.trash(
                ctx,
                SourceCommand(
                    observation_ids=(original.observation_id,),
                    reason="Withdraw original assertion A",
                ),
            )
            assert not await app.facts.query(ctx, FactQuery())
        finally:
            await app.close()

    asyncio.run(scenario())
