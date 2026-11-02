"""Small task outputs bind actual sources and publish readable long-term cognition."""

import asyncio
from pathlib import Path
from uuid import uuid4

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    QueryRequest,
)
from nous_sdk.contracts.growth import GrowthProposal
from nous_sdk.contracts.maintenance import MaintenanceRun, StartMaintenance

from nous.application import Application
from nous.cognition.growth_proposal import normalize_growth
from nous.maintenance.task_proposals import document
from nous.platform.configuration import Settings


def test_journal_and_dream_bind_sources_and_keep_hypotheses(tmp_path: Path):
    async def scenario():
        app = Application(Settings(instance=tmp_path))
        await app.authority.initialize()
        ctx = CallContext(caller_id="author", namespace_id=str(uuid4()), credential_source_ids=None)
        try:
            await app.identity.create_namespace(ctx, "long-term", "namespace")
            async with app.authority.write(ctx) as tx:
                admitted = await app.material.admit_text_in(
                    tx,
                    ctx,
                    ImportDocument(
                        text="The calibration completed after the filter was replaced.",
                        filename="calibration.txt",
                    ),
                )
            memory = await app.cognition.adopt(
                ctx,
                AdoptCognition(
                    operation_id="remember",
                    body=MemoryBody(text=admitted.material["text"]),
                    evidence=(EvidenceLink(observation_id=admitted.event["id"]),),
                ),
            )
            async with app.authority.read(ctx) as view:
                snapshot = await app.growth.growth_snapshot_in(view, ctx, ("memory",), 8)
            run = MaintenanceRun(
                id=str(uuid4()),
                namespace_id=ctx.namespace_id,
                created_by=ctx.caller_id,
                created_at="2026-11-01T00:00:00Z",
                job_id="task",
                command=StartMaintenance(
                    operation_id="journal", kind="journal", references=(memory.reference,)
                ),
            )
            journal = document(
                run,
                snapshot,
                {
                    "title": "Calibration notes",
                    "paragraphs": [
                        {
                            "text": "I finished the calibration after replacing the filter.",
                            "source_keys": [snapshot.entries[0].key],
                        }
                    ],
                },
                "retained-output",
            )
            draft = normalize_growth(
                GrowthProposal.model_validate(
                    journal.model_dump(include={"summary", "rationale", "actions"})
                )
            )
            async with app.authority.write(ctx) as tx:
                application = await app.growth.apply_growth_in(tx, ctx, draft, snapshot)
            written = application.records[0]
            assert written.body.kind == "journal"
            assert written.body.knowledge_cutoff == snapshot.watermark
            assert written.body.paragraph_sources[0].evidence[0].reference == memory.reference
            found = await app.retrieval.query(
                ctx, QueryRequest(text="calibration", mode="lexical", kinds=("journal",))
            )
            assert found.hits[0].object.reference == written.reference
            dream_run = run.model_copy(
                update={
                    "command": StartMaintenance(
                        operation_id="dream",
                        kind="dream",
                        instruction="Explore equipment reliability",
                    )
                }
            )
            hypothesis = document(
                dream_run,
                snapshot,
                {
                    "summary": "A maintenance hypothesis",
                    "rationale": "Selected experience",
                    "actions": [
                        {
                            "key": "h0",
                            "body": {
                                "kind": "schema",
                                "title": "Filter maintenance",
                                "claim": "Filter condition may affect calibration",
                                "scope": "the selected equipment",
                                "boundary": "requires further observations",
                                "support": [{"key": snapshot.entries[0].key}],
                                "epistemic": {"classification": "observed"},
                            },
                            "evidence_keys": [snapshot.entries[0].key],
                            "reason": "Explore a possible explanation",
                        }
                    ],
                    "questions": ["Does the pattern recur?"],
                },
                "retained-dream",
            )
            dream_draft = normalize_growth(
                GrowthProposal.model_validate(
                    hypothesis.model_dump(include={"summary", "rationale", "actions"})
                )
            )
            async with app.authority.write(ctx) as tx:
                application = await app.growth.apply_growth_in(tx, ctx, dream_draft, snapshot)
            assert application.records[0].body.epistemic.classification == "inferred"
            assert hypothesis.questions == ("Does the pattern recur?",)
        finally:
            await app.close()

    asyncio.run(scenario())


def test_journal_job_retains_local_input_while_model_is_unconfigured(tmp_path: Path):
    async def scenario():
        from nous_sdk.contracts import CreateJournal

        app = Application(Settings(instance=tmp_path))
        await app.start()
        ctx = CallContext(caller_id="host", namespace_id=str(uuid4()), credential_source_ids=None)
        try:
            await app.identity.create_namespace(ctx, "journal-job", "namespace")
            async with app.authority.write(ctx) as tx:
                admitted = await app.material.admit_text_in(
                    tx,
                    ctx,
                    ImportDocument(
                        text="The observed calibration finished.", filename="calibration.txt"
                    ),
                )
            memory = await app.cognition.adopt(
                ctx,
                AdoptCognition(
                    operation_id="memory",
                    body=MemoryBody(text=admitted.material["text"]),
                    evidence=(EvidenceLink(observation_id=admitted.event["id"]),),
                ),
            )
            submission = await app.maintenance.journal_create(
                ctx,
                CreateJournal(
                    operation_id="journal", references=(memory.reference,), style="work_review"
                ),
            )
            result = await app.execution.wait(ctx, submission.job_id)
            assert result["waiting_reason"] in {
                "model_unavailable",
                "model_not_configured",
                "configuration_missing",
            }
            run = await app.maintenance.read(ctx, submission.run_id)
            assert run.catalog is not None
            assert {entry.reference for entry in run.catalog.entries} == {memory.reference}
            assert run.status == "waiting"
            assert result["status"] == "waiting"
            assert run.waiting_reason is not None and run.draft is None
            assert not run.generation_call_ids
        finally:
            await app.close()

    asyncio.run(scenario())
