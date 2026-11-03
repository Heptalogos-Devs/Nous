"""Public Mapping imports preserve exact bytes, split policy, and real queued work."""

import asyncio

import httpx
import pytest
from nous_sdk import NousClient
from nous_sdk.imports import upload_import_inputs

from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_mapping_catalog_process_files_and_exact_upload(tmp_path):
    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            client = NousClient("http://test", credential_file=settings.token_file)
            await client.transport.http.aclose()
            client.transport.http = httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            )
            try:
                ns = await client.create_namespace("Legacy typed imports")
                subject = await client.create_subject(ns.id, name="Caller")
                scope = {"namespace_id": ns.id, "subject_id": subject.id}
                document = {
                    "name": "原样.txt",
                    "format": "text",
                    "input": {"kind": "inline", "content": "逐字🙂记录，不调用模型。"},
                    "source": "legacy://records/one",
                }
                catalog = await client.freeze_import_catalog(
                    {**scope, "inputs": [document], "overrides": {"strategy": "quote"}},
                    operation_id="legacy-freeze",
                )
                assert catalog.split_options.strategy_override == "quote"
                assert catalog.split_options.narrative_window_size == 1600
                assert catalog.units[0].context["source"] == "legacy://records/one"
                with pytest.raises(ValueError, match="explicit llm_enabled"):
                    await client.process_import_units({**scope, "catalog_id": catalog.id})
                submitted = await client.process_import_units(
                    {
                        **scope,
                        "catalog_id": catalog.id,
                        "unit_ids": [catalog.units[0].id],
                        "llm_enabled": False,
                        "extract_time": False,
                        "origin": {"conversation_ref": "process-chat", "external_object_ref": None},
                        "retry_parent_job_id": None,
                        "vectors": "disabled",
                    },
                    operation_id="legacy-process",
                )
                job = (
                    await client.wait_success(
                        (submitted.job_id,), namespace_id=ns.id, subject_id=subject.id
                    )
                )[0]
                assert job.output["output_ids"]
                processed = await client.import_unit_output(
                    ns.id, subject.id, job.output["output_ids"][0]
                )
                events = {
                    event.id: event for event in await client.source_directory(ns.id, subject.id)
                }
                assert processed.observation_id != catalog.units[0].observation_id
                assert events[processed.observation_id].origin == {
                    "conversation_ref": "process-chat"
                }
                assert events[catalog.units[0].observation_id].origin == {}

                accepted = await client.admit_import_files(
                    {
                        **scope,
                        "inputs": [{"document": document, "file_identity": "legacy:first"}],
                        "overrides": {"strategy": "quote"},
                        "llm_enabled": False,
                        "extract_time": False,
                        "origin": {"conversation_ref": "legacy-chat", "external_object_ref": None},
                        "vectors": "disabled",
                        "dedupe_policy": "none",
                    },
                    operation_id="legacy-file-job",
                )
                assert accepted.request_id and accepted.job_id
                await client.wait_success(
                    (accepted.job_id,), namespace_id=ns.id, subject_id=subject.id
                )
                original = tmp_path / "actual.json"
                data = b'{"records": []}\n'
                original.write_bytes(data)
                inputs = await upload_import_inputs(
                    client,
                    ns.id,
                    subject.id,
                    [{"path": str(original), "source": None}],
                    transfer_root=tmp_path,
                    input_format="text",
                    operation_id="opaque-original",
                )
                assert inputs[0].format == "text" and inputs[0].source is None
                downloaded = await client.download_material(
                    inputs[0].material_id,
                    tmp_path / "downloaded.json",
                    namespace_id=ns.id,
                    subject_id=subject.id,
                )
                assert downloaded.output.read_bytes() == data
                long_record = "逐字记录🙂保持完整。" * 600
                literal = await client.import_text(
                    {
                        **scope,
                        "inputs": [
                            {
                                "name": "complete.txt",
                                "format": "text",
                                "source": "legacy://whole-record",
                                "input": {"kind": "inline", "content": long_record},
                            }
                        ],
                        "form_memory": False,
                        "completion": "accepted",
                    },
                    operation_id="literal-record-boundary",
                )
                await client.wait_success(
                    (literal.job_id,), namespace_id=ns.id, subject_id=subject.id
                )
                literal_result = await client.text_import_result(
                    ns.id, subject.id, literal.request_id
                )
                assert literal_result.completion == "accepted"
                assert literal_result.formation_jobs == ()
                assert len(literal_result.admission.outputs) == 1
                assert literal_result.admission.catalog.units[0].text == long_record
                assert literal_result.admission.catalog.split_options.record_units is True
                with pytest.raises(ValueError, match="requires form_memory"):
                    await client.import_text(
                        {**scope, "inputs": [document], "completion": "formed"}
                    )
                deferred = await client.import_text(
                    {
                        **scope,
                        "inputs": [
                            {
                                **document,
                                "input": {"kind": "inline", "content": "等待形成的独立原始记录。"},
                            }
                        ],
                        "form_memory": True,
                        "completion": "accepted",
                        "vectors": "disabled",
                    },
                    operation_id="accepted-before-provider",
                )
                await client.wait_success(
                    (deferred.job_id,), namespace_id=ns.id, subject_id=subject.id
                )
                deferred_result = await client.text_import_result(
                    ns.id, subject.id, deferred.request_id
                )
                assert deferred_result.completion == "accepted"
                assert len(deferred_result.admission.outputs) == 1
                child = deferred_result.formation_jobs[0]
                # No formation model exists in this instance: the real semantic child fails,
                # while its parent retains the already completed literal admission.
                assert (
                    await client.wait_jobs(
                        (child.job_id,), namespace_id=ns.id, subject_id=subject.id
                    )
                )[0].status == "ERROR"
                calls = await client.request(
                    "GET",
                    "/api/model-calls",
                    params={"namespace_id": ns.id, "subject_id": subject.id},
                )
                assert calls == []
            finally:
                await client.close()

    asyncio.run(scenario())
