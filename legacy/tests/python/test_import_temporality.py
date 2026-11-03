"""Actual SDK imports preserve explicit observation/applicability and temporal record versions."""

import asyncio
import json

import httpx
from nous_sdk import NousClient
from nous_sdk.contracts import AdmitFiles, ImportDocument, TimePoint, TimeRange
from sqlalchemy import select

from nous.material.service import observations
from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_sdk_document_and_record_time_overrides_survive_catalog_and_formation(tmp_path):
    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            async with NousClient("http://test", credential_file=settings.token_file) as api:
                await api.transport.http.aclose()
                api.transport.http = httpx.AsyncClient(
                    transport=httpx.ASGITransport(app=app), base_url="http://test"
                )
                namespace = await api.create_namespace("Temporal source declarations")
                subject = await api.create_subject(namespace.id)
                first = ImportDocument(
                    text="Declared content",
                    observed_at=TimePoint(coordinate=100),
                    event_time=TimePoint(coordinate=50),
                    validity=TimeRange(start=10, end=20),
                )
                values = []
                for document in (
                    first,
                    first.model_copy(
                        update={
                            "observed_at": TimePoint(coordinate=200),
                            "validity": TimeRange(start=30, end=40),
                        }
                    ),
                ):
                    result = await api.admit_documents(
                        namespace.id,
                        subject.id,
                        AdmitFiles(inputs=(document,), llm_enabled=False, vectors="disabled"),
                    )
                    assert result.outputs and not result.skipped_success_ids
                    values.append(
                        (
                            await api.record_detail(
                                namespace.id, subject.id, result.outputs[0].cognition[0]
                            )
                        ).entry.record
                    )
                assert [record.times.observation.coordinate for record in values] == [100, 200]
                assert [record.times.validity for record in values] == [
                    TimeRange(start=10, end=20),
                    TimeRange(start=30, end=40),
                ]
                assert all(
                    record.times.event.coordinate == 50 and record.times.recorded.coordinate > 200
                    for record in values
                )
                content = json.dumps(
                    [
                        {
                            "text": "First record",
                            "observed_at": {"coordinate": 300},
                            "validity": {"start": 1, "end": 2},
                        },
                        {
                            "text": "Second record",
                            "observed_at": {"coordinate": 400},
                            "validity": None,
                        },
                    ]
                )
                result = await api.admit_documents(
                    namespace.id,
                    subject.id,
                    AdmitFiles(
                        inputs=(
                            ImportDocument(
                                text=content,
                                format="json",
                                observed_at=TimePoint(coordinate=500),
                                validity=TimeRange(start=1000, end=2000),
                            ),
                        ),
                        record_units=True,
                        llm_enabled=False,
                        vectors="disabled",
                    ),
                )
                records = [
                    (
                        await api.record_detail(namespace.id, subject.id, output.cognition[0])
                    ).entry.record
                    for output in result.outputs
                ]
                assert [record.times.observation.coordinate for record in records] == [300, 400]
                assert records[0].times.validity == TimeRange(start=1, end=2)
                assert records[1].times.validity is None
                service = app.state.application
                async with service.authority.read() as view:
                    accepted = (
                        await view.connection.execute(
                            select(observations.c.record).where(
                                observations.c.id == result.outputs[1].observation_id
                            )
                        )
                    ).scalar_one()
                assert "validity" in accepted and accepted["validity"] is None

    asyncio.run(scenario())
