"""Saved annotation proposals remain available across adoption failures."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from unittest.mock import patch

import pytest

from nous.application.configuration import open_configuration_operations
from nous.cognition.annotation.contracts import (
    AdoptAnnotation,
    AnnotateRecord,
    ProposeAnnotation,
    ReadAnnotationProposal,
)
from nous.cognition.annotation.operations import (
    ADOPT_ANNOTATION,
    ANNOTATE_RECORD,
    PROPOSE_ANNOTATION,
    READ_ANNOTATION_PROPOSAL,
)
from nous.cognition.concept_contracts import CreateConcept
from nous.cognition.concepts import CREATE_CONCEPT
from nous.cognition.writing.contracts import (
    AddNote,
    AfterWrite,
    CreateJournal,
    ReadRecord,
    ReplaceRecord,
)
from nous.cognition.writing.operations import ADD_NOTE, CREATE_JOURNAL, READ_RECORD, REPLACE_RECORD
from nous.common.operations import exception_context
from nous.content.annotations import AnnotationProposal
from nous.content.contracts import InlineText, ResultRef
from nous.runtime.bootstrap import initialize_instance, open_instance
from tests.integration.shared.models import configure_models, model_server


def test_saved_proposal_can_be_explicitly_adopted_after_failure_and_reopen(tmp_path: Path) -> None:
    calls: list[object] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            calls.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            proposal = {
                "concepts": [
                    {"key": "K", "label": "Meaning", "description": "A meaning", "reuse_key": None}
                ],
                "occurrences": [],
                "paths": [],
            }
            data = json.dumps(
                {
                    "id": "annotation",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": json.dumps(proposal)},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "annotation"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "tasks.annotate-concepts": {
                            "model": "reader",
                            "prompt": "annotate-concepts",
                            "output_mode": "native",
                        }
                    }
                )
            async with open_instance(instance, environment={}) as runtime:
                record = await runtime.app.call(
                    CREATE_JOURNAL,
                    CreateJournal(
                        body=InlineText(text="# Title\n\nA body."),
                        after=AfterWrite(annotation="skip"),
                    ),
                )
                concept = await runtime.app.call(
                    CREATE_CONCEPT, CreateConcept(label="Existing", description="Existing meaning")
                )
                with patch.object(
                    runtime.annotations, "adopt", side_effect=OSError("injected adoption failure")
                ):
                    with pytest.raises(OSError) as failure:
                        await runtime.app.call(
                            ANNOTATE_RECORD,
                            AnnotateRecord(
                                target=record.commit.revision_ref, bindings={"K": concept.ref}
                            ),
                        )
                proposal_ref = ResultRef.model_validate(
                    exception_context(failure.value)["proposal_result"]
                )
                assert len(calls) == 1

            async with open_instance(instance, environment={}) as runtime:
                proposal = await runtime.app.call(
                    READ_ANNOTATION_PROPOSAL, ReadAnnotationProposal(ref=proposal_ref)
                )
                assert proposal.body_revision == record.commit.revision_ref
                request = AdoptAnnotation(
                    target=record.commit.revision_ref,
                    proposal=proposal_ref,
                    bindings={"K": concept.ref},
                )
                adopted = await runtime.app.call(ADOPT_ANNOTATION, request)
                repeated = await runtime.app.call(ADOPT_ANNOTATION, request)
                assert adopted.ref == repeated.ref
                assert adopted.bindings["K"].ref == concept.ref
                assert len(calls) == 1

                stale = await runtime.app.call(
                    ANNOTATE_RECORD,
                    AnnotateRecord(target=record.commit.revision_ref, draft_only=True),
                )
                assert stale.state == "proposed"
                await runtime.app.call(
                    REPLACE_RECORD,
                    ReplaceRecord(
                        target=record.commit.revision_ref,
                        body=InlineText(text="# Title\n\nA new body."),
                        after=AfterWrite(annotation="skip"),
                    ),
                )
                outcome = await runtime.app.call(
                    ANNOTATE_RECORD,
                    AnnotateRecord(
                        target=record.commit.revision_ref, proposal_result=stale.proposal_ref
                    ),
                )
                assert outcome.state == "superseded"
                assert (
                    await runtime.app.call(
                        READ_ANNOTATION_PROPOSAL, ReadAnnotationProposal(ref=stale.proposal_ref)
                    )
                ).body_revision == record.commit.revision_ref
                assert len(calls) == 2

        asyncio.run(scenario())


def test_manual_annotation_reuses_an_inactive_adoption_and_activates_once(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "manual-annotation"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            note = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A recalled note."),
                    remember=True,
                    after=AfterWrite(annotation="skip"),
                ),
            )
            proposal = await runtime.app.call(
                PROPOSE_ANNOTATION,
                ProposeAnnotation(
                    target=note.commit.revision_ref,
                    proposal=AnnotationProposal.model_validate(
                        {
                            "concepts": [
                                {"key": "m", "label": "Manual", "description": "Manual concept"}
                            ],
                            "occurrences": [{"key": "o", "concept": "m", "blocks": ["B1"]}],
                            "paths": [],
                        }
                    ),
                ),
            )
            inactive = await runtime.app.call(
                ADOPT_ANNOTATION,
                AdoptAnnotation(
                    target=note.commit.revision_ref, proposal=proposal.result_ref, activate=False
                ),
            )
            assert not inactive.active
            before = (
                await runtime.app.call(READ_RECORD, ReadRecord(target=note.commit.ref))
            ).namespace.content_generation
            active = await runtime.app.call(
                ADOPT_ANNOTATION,
                AdoptAnnotation(target=note.commit.revision_ref, proposal=proposal.result_ref),
            )
            assert active.ref == inactive.ref and active.active
            after = (
                await runtime.app.call(READ_RECORD, ReadRecord(target=note.commit.ref))
            ).namespace.content_generation
            assert after == before + 1
            repeated = await runtime.app.call(
                ADOPT_ANNOTATION,
                AdoptAnnotation(target=note.commit.revision_ref, proposal=proposal.result_ref),
            )
            assert repeated.ref == active.ref
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=note.commit.ref))
            ).namespace.content_generation == after

    asyncio.run(scenario())
