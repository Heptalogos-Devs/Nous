"""Model protocol and retrieval through the real SDK and CLI."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path

from pydantic import JsonValue, TypeAdapter

from nous.application.configuration import open_configuration_operations
from tests.integration.shared.models import (
    configure_models,
    model_server,
)


def test_hybrid_query_preserves_context_selection_and_feedback(tmp_path: Path) -> None:
    """A real Wave query consumes immutable context settings and retains its exact source."""
    from nous.application.inputs import FileInput
    from nous.cognition.feedback import (
        READ_FEEDBACK,
        RECORD_QUERY_FEEDBACK,
        QueryFeedbackRequest,
        ReadFeedback,
    )
    from nous.cognition.retrieval.contracts import (
        ContextSelection,
        EvidenceDelivery,
        QueryControls,
        QueryRequest,
    )
    from nous.cognition.retrieval.operations import QUERY
    from nous.cognition.writing.contracts import AddNote, ReadRecord
    from nous.cognition.writing.operations import ADD_NOTE, READ_RECORD
    from nous.content.contracts import (
        ContentSelection,
        ExternalInput,
        InlineText,
        ResultOutputRef,
        SourceSelection,
    )
    from nous.runtime.bootstrap import open_instance
    from nous.temporal.values import Instant, Interval, Known, TimePoint, parse_explicit_time
    from nous.wave.contracts import (
        CandidateFilter,
        ContextContent,
        ContextFragment,
        TimeCondition,
        WaveBuild,
        WaveQuery,
    )
    from nous.wave.operations import WAVE_BUILD, WAVE_QUERY

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = TypeAdapter(dict[str, JsonValue]).validate_json(
                self.rfile.read(int(self.headers["Content-Length"]))
            )
            inputs = request["input"]
            count = len(inputs) if isinstance(inputs, list) else 1
            body = json.dumps(
                {
                    "object": "list",
                    "model": "local-embedding",
                    "data": [
                        {"object": "embedding", "index": index, "embedding": [0.5, 0.25]}
                        for index in range(count)
                    ],
                    "usage": {"prompt_tokens": count, "total_tokens": count},
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "context-feedback"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update({"cognition.search.space": "test-space"})
            async with open_instance(instance, environment={}) as runtime:
                note = await runtime.app.call(
                    ADD_NOTE,
                    AddNote(
                        body=InlineText(text="Moonlight reflects borrowed light."), remember=True
                    ),
                )
                view = await runtime.app.call(
                    READ_RECORD, ReadRecord(target=note.commit.revision_ref)
                )
                selector = SourceSelection(start_byte=0, end_byte=9)
                selected = ContentSelection(artifact=view.body_artifact, selection=selector)
                request = QueryRequest(
                    text="How can help become a source of light?",
                    save=True,
                    context=ContextSelection(
                        fragments=(
                            ContextFragment(
                                key="source-excerpt",
                                role="user",
                                sequence=1,
                                content=ContextContent(
                                    artifact=view.body_artifact.identifier, selector=selector
                                ),
                            ),
                        )
                    ),
                )
                answer = await runtime.app.call(QUERY, request)
                assert answer.evidence.materials
                assert answer.evidence.intent.context[0].source is not None
                assert answer.evidence.intent.context[0].source.selections == (selected,)
                recorded = view.times.recorded
                assert recorded is not None
                window = Interval(
                    start=Known(at=recorded),
                    end=Known(at=recorded.model_copy(update={"ticks": recorded.ticks + 1})),
                    meaning="duration",
                )
                filtered = await runtime.app.call(
                    QUERY,
                    QueryRequest(
                        text="Moonlight",
                        controls=QueryControls(
                            filters=CandidateFilter(
                                times=(TimeCondition(axis="recorded", range=window),)
                            )
                        ),
                    ),
                )
                assert [group.material.ref for group in filtered.evidence.materials] == [
                    note.commit.revision_ref
                ]
                # Manual collections cannot replace the artifact's actual recorded time.
                manifest = tmp_path / "manual-time.json"
                manifest.write_text(
                    json.dumps(
                        {
                            "name": "manual-recorded-time",
                            "documents": [
                                {
                                    "key": "body",
                                    "input": {"artifact": view.body_artifact.identifier},
                                    "times": {"recorded": {"domain_id": "utc", "ticks": 0}},
                                }
                            ],
                            "candidates": [
                                {"key": "body", "document": "body", "content": {"document": "body"}}
                            ],
                        }
                    ),
                    encoding="utf-8",
                )
                built = await runtime.app.call(
                    WAVE_BUILD,
                    WaveBuild(manifest=ExternalInput(key="manifest")),
                    inputs={"manifest": FileInput(manifest)},
                )
                snapshot = built.snapshot.identifier
                epoch = TimePoint(domain_id="utc", ticks=0)
                spoofed = Interval(
                    start=Known(at=epoch),
                    end=Known(at=epoch.model_copy(update={"ticks": 1})),
                    meaning="duration",
                )

                def temporal_filter(value: Interval) -> CandidateFilter:
                    return CandidateFilter(times=(TimeCondition(axis="recorded", range=value),))

                old = await runtime.app.call(
                    WAVE_QUERY,
                    WaveQuery(
                        snapshot=snapshot,
                        text="Moonlight",
                        program="lexical",
                        candidate_filter=temporal_filter(spoofed),
                    ),
                )
                assert not old.hits
                artifact = await runtime.artifacts.get(view.body_artifact)
                actual = parse_explicit_time(artifact.recorded_at.isoformat())
                assert isinstance(actual, Instant)
                actual_window = Interval(
                    start=Known(at=actual.at),
                    end=Known(at=actual.at.model_copy(update={"ticks": actual.at.ticks + 1})),
                    meaning="duration",
                )
                current = await runtime.app.call(
                    WAVE_QUERY,
                    WaveQuery(
                        snapshot=snapshot,
                        text="Moonlight",
                        program="lexical",
                        candidate_filter=temporal_filter(actual_window),
                    ),
                )
                assert len(current.hits) == 1
                assert answer.evidence_ref is not None
                feedback = await runtime.app.call(
                    RECORD_QUERY_FEEDBACK,
                    QueryFeedbackRequest(
                        evidence=answer.evidence_ref,
                        category="retrieval",
                        passage_key=answer.evidence.passages[0].key,
                    ),
                )
            async with open_instance(instance, environment={}) as reopened:
                stored = await reopened.app.call(
                    READ_FEEDBACK, ReadFeedback(ref=feedback.identifier)
                )
                assert stored.target.query is not None
                assert stored.target.query.evidence == answer.evidence_ref
                artifact = await reopened.results.resolve_output(
                    ResultOutputRef(result=answer.evidence_ref, output="evidence")
                )
                delivery = EvidenceDelivery.model_validate_json(
                    await reopened.artifacts.read_text(artifact)
                )
                assert delivery.intent.context == answer.evidence.intent.context

        asyncio.run(scenario())
