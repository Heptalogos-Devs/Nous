"""Seed interpretation keeps real project materials separate from adopted cognition."""

import asyncio
from pathlib import Path

import httpx
import pytest
from nous_sdk import NousClient
from nous_sdk.contracts import CallContext, DomainError, ImportDocument
from nous_sdk.contracts.ego_model import InterpretSeed
from nous_sdk.contracts.growth import GrowthAdoption, GrowthDraft
from nous_sdk.contracts.maintenance import (
    EditMaintenanceProposal,
    MaintenanceProposal,
    ReviewMaintenance,
)
from sqlalchemy import func, select

from nous.bootstrap import application
from nous.cognition.schema import revisions
from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_interpretation_admits_sources_without_memory_and_adoption_uses_material_evidence(tmp_path):
    async def scenario():
        async with application(Settings(instance=tmp_path / "instance")) as app:
            ctx = CallContext(
                caller_id="owner",
                namespace_id="ns",
                subject_id="subject",
                credential_source_ids=None,
            )
            await app.identity.create_namespace(ctx, "Project", "ns")
            await app.identity.create_subject(
                ctx, "Project", "subject", subject_id="subject", capabilities=("memory", "ego")
            )
            text = await asyncio.to_thread(Path("README.md").read_text, encoding="utf-8")
            command = InterpretSeed(
                operation_id="interpret-project",
                documents=(ImportDocument(text=text, filename="README.md"),),
            )
            submission = await app.seeds.interpret(ctx, command)
            with pytest.raises(DomainError) as unavailable:
                await app.execution.wait(ctx, submission.job_id)
            assert unavailable.value.code == "model_unavailable", str(unavailable.value)
            run = await app.maintenance.read(ctx, submission.run_id)
            assert run.catalog.sources and not run.catalog.entries
            async with app.authority.read(ctx) as view:
                assert (
                    await view.connection.scalar(select(func.count()).select_from(revisions)) == 0
                )
                snapshot = await app.maintenance.snapshot_in(view, ctx, run.catalog)
            assert text.strip() in snapshot.sources[0].text
            assert await app.models.ledger.list(ctx) == []
            draft = GrowthDraft(
                summary="Adopt selected project knowledge",
                rationale="Explicit local adoption",
                actions=(
                    GrowthAdoption(
                        action="adopt",
                        key="chosen",
                        body_input={
                            "kind": "memory",
                            "text": text.splitlines()[2],
                            "epistemic": {"classification": "reported"},
                        },
                        evidence_keys=(snapshot.sources[0].key,),
                        reason="Actual project document",
                    ),
                ),
            )
            async with app.authority.write(
                ctx, "adopt-selected", operation_kind="growth.review", payload=draft
            ) as tx:
                adopted = (await app.growth.apply_growth_in(tx, ctx, draft, snapshot)).records
                tx.result(adopted)
            assert adopted[0].body.epistemic.classification == "reported"
            assert adopted[0].source_roots == (snapshot.sources[0].evidence[0].observation_id,)
            assert await app.seeds.interpret(ctx, command) == submission
            web = create_app(app.settings)
            web.state.application = app
            async with NousClient("http://test", credential_file=app.settings.token_file) as api:
                await api.transport.http.aclose()
                api.transport.http = httpx.AsyncClient(
                    transport=httpx.ASGITransport(app=web), base_url="http://test"
                )
                first = {
                    "action": "adopt",
                    "key": "original",
                    "body": {
                        "kind": "memory",
                        "text": text.splitlines()[2],
                        "epistemic": {"classification": "reported"},
                    },
                    "evidence_keys": [snapshot.sources[0].key],
                    "reason": "README source",
                }
                dependent = {**first, "key": "dependent", "evidence_keys": ["original"]}
                invalid = {**first, "key": "invalid", "body": {**first["body"], "unexpected": True}}
                run = await api.edit_maintenance(
                    "ns",
                    "subject",
                    run.id,
                    EditMaintenanceProposal(
                        operation_id="edit-readme",
                        expected_revision=run.revision,
                        proposal=MaintenanceProposal(
                            summary="README",
                            rationale="Selected actual README text",
                            actions=(first, dependent, invalid),
                        ),
                        reason="Explicit source-backed edit",
                    ),
                )
                assert {group.key: group.status for group in run.groups} == {
                    "original": "ready",
                    "invalid": "failed",
                    "dependent": "ready",
                }
                run = await api.review_maintenance(
                    "ns",
                    "subject",
                    run.id,
                    ReviewMaintenance(
                        operation_id="adopt-dependent",
                        expected_revision=run.revision,
                        decision="adopt",
                        group_keys=("dependent",),
                    ),
                )
                assert {group.key: group.status for group in run.groups} == {
                    "original": "adopted",
                    "invalid": "failed",
                    "dependent": "adopted",
                }
                original = next(group for group in run.groups if group.key == "original").adopted[0]
                revision = {**first, "key": "revision", "target_key": "original"}
                run = await api.edit_maintenance(
                    "ns",
                    "subject",
                    run.id,
                    EditMaintenanceProposal(
                        operation_id="append-readme-revision",
                        expected_revision=run.revision,
                        proposal=run.proposal.model_copy(
                            update={"actions": (*run.proposal.actions, revision)}
                        ),
                        reason="Explicit same-identity revision",
                    ),
                )
                run = await api.review_maintenance(
                    "ns",
                    "subject",
                    run.id,
                    ReviewMaintenance(
                        operation_id="adopt-readme-revision",
                        expected_revision=run.revision,
                        decision="adopt",
                        group_keys=("revision",),
                    ),
                )
                revised = next(group for group in run.groups if group.key == "revision").adopted[0]
                assert revised.id == original.id and revised.revision == original.revision + 1
                assert run.edits[-1].reason == "Explicit same-identity revision"
                assert await app.models.ledger.list(ctx) == []

    asyncio.run(scenario())
