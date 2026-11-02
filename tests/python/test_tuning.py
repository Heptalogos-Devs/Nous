"""Durable tuning, caller visibility and profile rollback through the public SDK."""

import asyncio
from pathlib import Path

import httpx
from nous_sdk import NousClient
from nous_sdk.contracts import AdmitFiles, ImportDocument, QueryRequest, SourceCommand
from nous_sdk.contracts.tuning import (
    ApplyRetrievalProfile,
    CreateTuning,
    RollbackRetrievalProfile,
    TuningCase,
    TuningParameter,
)

from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_tuning_job_and_rollback_retain_the_actual_preceding_profile(tmp_path):
    text = (
        (Path(__file__).resolve().parents[2] / "README.md")
        .read_text(encoding="utf-8")
        .split("\n\n")[0]
    )

    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            api = NousClient("http://test", credential_file=settings.token_file)
            await api.transport.http.aclose()
            api.transport.http = httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            )
            try:
                ns = await api.create_namespace("Tuning")
                subject = await api.create_subject(ns.id)
                admitted = await api.admit_documents(
                    ns.id,
                    subject.id,
                    AdmitFiles(
                        inputs=(ImportDocument(text=text, filename="README.md"),),
                        llm_enabled=False,
                        vectors="disabled",
                    ),
                )
                reference = admitted.outputs[0].cognition[0]
                baseline = await api.retrieval_profile(ns.id, subject.id)
                task = await api.create_tuning(
                    ns.id,
                    subject.id,
                    CreateTuning(
                        operation_id="exact-reference-tuning",
                        cases=(
                            TuningCase(
                                id="exact-reference",
                                query=QueryRequest(reference=reference.exact()),
                                relevance={reference.exact(): 1},
                            ),
                        ),
                        parameters=(TuningParameter(name="rrf_k", choices=(10, 60)),),
                        max_trials=3,
                    ),
                )
                await api.wait_success((task.job_id,), namespace_id=ns.id, subject_id=subject.id)
                task = await api.tuning_task(ns.id, subject.id, task.id)
                assert task.status == "SUCCESS" and task.completed_trials == task.total_trials
                measurements = (await api.tuning_rounds(ns.id, subject.id, task.id)).items
                assert all(item.queries[0].ranking == (reference.exact(),) for item in measurements)
                assert task.request is not None and task.holdout == ()
                chosen = measurements[-1].profile
                await api.apply_retrieval_profile(
                    ns.id,
                    subject.id,
                    ApplyRetrievalProfile(
                        operation_id="apply-measured",
                        profile=chosen,
                    ),
                )
                parent = chosen.model_copy(update={"rrf_k": 123}).model_dump(mode="json")
                await api.configuration(
                    {
                        "action": "set",
                        "namespace_id": ns.id,
                        "scope": "namespace",
                        "key": "retrieval",
                        "value": parent,
                    }
                )
                await api.rollback_retrieval_profile(
                    ns.id, subject.id, RollbackRetrievalProfile(operation_id="rollback-measured")
                )
                assert (await api.retrieval_profile(ns.id, subject.id))["value"] == baseline[
                    "value"
                ]
                await api.purge_sources(
                    ns.id,
                    subject.id,
                    SourceCommand(
                        operation_id="erase-input",
                        observation_ids=(admitted.outputs[0].observation_id,),
                        reason="Remove the retained document",
                    ),
                )
                erased = await api.tuning_task(ns.id, subject.id, task.id)
                assert erased.purged and erased.request is None
                assert not (await api.tuning_rounds(ns.id, subject.id, task.id)).items
            finally:
                await api.close()

    asyncio.run(scenario())
