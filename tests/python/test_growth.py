"""Pure local-key proposals validate without writes before actual atomic adoption."""

import asyncio
import json
from pathlib import Path

from nous_sdk.contracts import AdmitFiles, CallContext, DomainError, ImportDocument
from nous_sdk.contracts.growth import GrowthProposal
from sqlalchemy import func, select

from nous.cognition.growth_proposal import growth_model_catalog, normalize_growth
from nous.cognition.schema import revisions
from nous.cognition.service import CognitionService
from nous.identity.service import IdentityService
from nous.imports.service import ImportService
from nous.platform.persistence import Authority


class LiteralModels:
    async def form(self, *args, **kwargs):
        raise AssertionError("Local-key validation never calls a model")

    async def embed(self, *args, **kwargs):
        raise AssertionError("Local-key validation never embeds")


def test_pure_growth_and_actual_adoption(tmp_path: Path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        cognition = CognitionService(authority)
        imports = ImportService(authority, cognition, LiteralModels())
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None, caller_id="owner", namespace_id="ns", subject_id="subject"
        )
        try:
            await identity.create_namespace(ctx, "namespace", "namespace")
            await identity.create_subject(ctx, "subject", "subject", subject_id="subject")
            command = AdmitFiles(
                operation_id="original",
                inputs=(ImportDocument(text="Growth source.", filename="source.txt"),),
                llm_enabled=False,
                vectors="disabled",
            )
            imported = await imports.admit_files(ctx, command)
            memory = imported.outputs[0].cognition[0]
            async with authority.read(ctx) as view:
                snapshot = await cognition.growth_snapshot_in(view, ctx)
                count = await view.connection.scalar(select(func.count()).select_from(revisions))
            memory_key = next(
                entry.key for entry in snapshot.entries if entry.record.reference == memory
            )
            model_input = json.dumps(growth_model_catalog(snapshot))
            assert memory.id not in model_input
            assert imported.outputs[0].observation_id not in model_input
            proposal = GrowthProposal.model_validate(
                {
                    "summary": "Grounded schema",
                    "rationale": "Source-backed generalization",
                    "actions": [
                        {
                            "action": "adopt",
                            "key": "schema",
                            "evidence_keys": [memory_key],
                            "reason": "generalize",
                            "body": {
                                "kind": "schema",
                                "title": "Growth",
                                "claim": "Growth requires sources",
                                "scope": "this source",
                                "boundary": "no broader claim",
                                "support": [{"key": memory_key}],
                                "epistemic": {"classification": "derived"},
                            },
                        },
                        {
                            "action": "adopt",
                            "key": "concept",
                            "evidence_keys": [memory_key],
                            "reason": "organize",
                            "body": {
                                "kind": "concept",
                                "label": "Growth",
                                "members": [{"key": "schema"}],
                            },
                        },
                    ],
                }
            )
            draft = normalize_growth(proposal)
            async with authority.read(ctx) as view:
                await cognition.validate_growth_in(view, ctx, draft, snapshot)
                assert view.commit_seq == snapshot.watermark
                assert (
                    await view.connection.scalar(select(func.count()).select_from(revisions))
                    == count
                )
            async with authority.write(
                ctx, "growth", operation_kind="growth-review", payload=draft
            ) as tx:
                adopted = await cognition.apply_growth_in(tx, ctx, draft, snapshot)
                tx.result(adopted)
            assert adopted[0].body.support == (memory,)
            assert adopted[1].body.members == (adopted[0].reference,)
            assert all(not record.reference.id.startswith("proposal/") for record in adopted)
            denied = ctx.model_copy(update={"allowed_source_ids": ()})
            try:
                await imports.admit_files(denied, command)
            except DomainError as error:
                assert error.code == "source_unavailable"
            else:
                raise AssertionError("Cached original text cannot bypass the current source scope")
        finally:
            await authority.close()

    asyncio.run(scenario())
