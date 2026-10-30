"""Directory recovery preserves saved readable names and current source visibility."""

import asyncio
from pathlib import Path

import pytest
from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    DomainError,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
)

from nous.cognition.service import CognitionService
from nous.identity.service import IdentityService
from nous.material.service import MaterialService
from nous.platform.persistence import Authority
from nous.platform.persistence.authority import canonical_json
from nous.references.allocation import targets
from nous.references.schema import addresses
from nous.references.service import ReferenceDirectory


def test_reconcile_recovers_old_names_only_within_visible_sources(tmp_path: Path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        identity = IdentityService(authority)
        materials = MaterialService(authority)
        cognition = CognitionService(authority, materials=materials)
        directory = ReferenceDirectory(authority, cognition, materials)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None,
            caller_id="owner",
            namespace_id="research",
            subject_id="subject",
        )
        try:
            await identity.create_namespace(ctx, "Research", "namespace")
            await identity.create_subject(ctx, "Researcher", "subject", subject_id=ctx.subject_id)
            async with authority.write(
                ctx, "adopt", operation_kind="test.adopt", scope=ctx.require_subject(), payload=None
            ) as tx:
                records = []
                for text in ("可读的独立资料。", "当前范围以外的独立资料。"):
                    document = ImportDocument(text=text)
                    material = await materials.put_in(tx, ctx, document)
                    observation = await materials.observe_in(tx, ctx, material["id"], document)
                    records.append(
                        await cognition.adopt_in(
                            tx,
                            ctx,
                            AdoptCognition(
                                body=MemoryBody(text=text),
                                evidence=(EvidenceLink(observation_id=observation["id"]),),
                            ),
                        )
                    )
                tx.result(records)
            first, second = records
            assert first.addresses is not None and second.addresses is not None
            async with authority.write(
                ctx, "lost-directory", operation_kind="test.directory_loss", payload=None
            ) as tx:
                encoded = [
                    canonical_json(target)
                    for record in records
                    for target in targets(record.reference)
                ]
                await tx.connection.execute(
                    addresses.delete().where(addresses.c.target.in_(encoded))
                )
                tx.result(None)
            limited = ctx.model_copy(update={"allowed_source_ids": (first.reference.exact(),)})
            recovered = await directory.reconcile(limited, "limited-recovery")
            assert recovered["cognition"] == 1
            assert await directory.reconcile(limited, "limited-recovery") == recovered
            assert (
                await directory.resolve(limited, first.addresses.exact)
            ).reference.reference == first.reference
            with pytest.raises(DomainError) as missing:
                await directory.resolve(ctx, second.addresses.exact)
            assert missing.value.code == "reference_not_found"
            assert (await directory.reconcile(ctx, "full-recovery"))["cognition"] == 1
            assert (
                await directory.resolve(ctx, second.addresses.exact)
            ).reference.reference == second.reference
        finally:
            await authority.close()

    asyncio.run(scenario())
