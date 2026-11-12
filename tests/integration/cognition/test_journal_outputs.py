"""Explicit no-material output preserves computation without creating written content."""

import asyncio
from pathlib import Path

import pytest

from nous.common.errors import NousError
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_no_material_and_draft_corrections_use_real_writing_path(tmp_path: Path) -> None:
    import json
    from http.server import BaseHTTPRequestHandler

    from nous.application.configuration import open_configuration_operations
    from nous.cognition.contracts import RevisionSelection
    from nous.cognition.retrieval.contracts import CognitiveQuery
    from nous.cognition.retrieval.operations import QUERY
    from nous.cognition.writing.contracts import (
        AddNote,
        AdoptDraft,
        AfterWrite,
        DraftFromResult,
        EditDraftRequest,
        ListRecords,
        NewDraftRequest,
        ReadDraft,
        ReadRecord,
        ReviseDraft,
        SourceChoice,
    )
    from nous.cognition.writing.generation import (
        DRAFT_FROM_RESULT,
        READ_DRAFT,
        REVISE_DRAFT,
        WRITE_DRAFT,
    )
    from nous.cognition.writing.operations import ADD_NOTE, ADOPT_DRAFT, LIST_RECORDS, READ_RECORD
    from nous.content.contracts import ContentSelection, InlineText, ResultRef, SourceSelection
    from tests.integration.shared.models import model_server

    answer = ""
    requests: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            payload = json.dumps(
                {
                    "id": "writing",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": answer},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    async def scenario() -> None:
        nonlocal answer
        instance = tmp_path / "journal-output"
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update(
                {
                    "providers.local": {
                        "protocol": "openai-chat-completions",
                        "endpoint": f"http://127.0.0.1:{server.server_port}/v1",
                    },
                    "models.reader": {
                        "provider": "local",
                        "model_name": "local",
                        "capabilities": ["text", "structured_output"],
                        "output_modes": ["text", "native"],
                    },
                    "tasks.journal-draft": {
                        "model": "reader",
                        "prompt": "journal-draft",
                        "output_mode": "text",
                    },
                    "tasks.journal-edit": {
                        "model": "reader",
                        "prompt": "journal-edit",
                        "output_mode": "native",
                    },
                }
            )
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            note = await runtime.app.call(
                ADD_NOTE, AddNote(body=InlineText(text="你好。"), after=policy)
            )
            source = SourceChoice(target=note.commit.revision_ref)
            answer = "\n !NO_MATERIAL!\t"
            outcome = await runtime.app.call(WRITE_DRAFT, NewDraftRequest(sources=(source,)))
            assert outcome.status == "no_material" and outcome.draft is None
            product = await runtime.results.get(outcome.product)
            output = next(item.artifact for item in product.outputs if item.name == "output")
            assert await runtime.artifacts.read_text(output) == answer
            assert await runtime.drafts.status() == {"drafts": 0, "unadopted": 0}
            assert await runtime.app.call(LIST_RECORDS, ListRecords(kind="journal")) == ()

            for invalid_answer in ("", " \n\t"):
                answer = invalid_answer
                with pytest.raises(NousError) as failed:
                    await runtime.app.call(WRITE_DRAFT, NewDraftRequest(sources=(source,)))
                assert failed.value.code == "model.empty_output"

            answer = "# 标记说明\n\n!NO_MATERIAL! 出现在正常正文中。"
            drafted = await runtime.app.call(WRITE_DRAFT, NewDraftRequest(sources=(source,)))
            assert drafted.draft is not None
            draft = await runtime.app.call(READ_DRAFT, ReadDraft(draft=drafted.draft))
            assert draft.body == answer
            saved = await runtime.app.call(ADOPT_DRAFT, AdoptDraft(draft=draft.ref, after=policy))
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=saved.commit.revision_ref))
            ).body == answer

            answer = "# 引用待修\n\n[材料](source:missing)"
            invalid = await runtime.app.call(WRITE_DRAFT, NewDraftRequest(sources=(source,)))
            assert invalid.draft is not None
            invalid_view = await runtime.app.call(READ_DRAFT, ReadDraft(draft=invalid.draft))
            assert invalid_view.context.issues[0].code == "cognition.source_unknown"
            with pytest.raises(NousError) as failed:
                await runtime.app.call(ADOPT_DRAFT, AdoptDraft(draft=invalid.draft, after=policy))
            assert failed.value.code == "cognition.draft_invalid"
            source_key = invalid_view.context.source_bindings[0].key
            corrected = await runtime.app.call(
                REVISE_DRAFT,
                ReviseDraft(
                    draft=invalid.draft,
                    markdown=InlineText(text=f"# 引用已修\n\n[材料](source:{source_key})"),
                ),
            )
            assert corrected.parent_draft == invalid.draft
            adopted = await runtime.app.call(
                ADOPT_DRAFT, AdoptDraft(draft=corrected.ref, after=policy)
            )
            retried = await runtime.app.call(
                ADOPT_DRAFT, AdoptDraft(draft=corrected.ref, after=policy)
            )
            assert (
                retried.commit.revision_ref == adopted.commit.revision_ref
                and not retried.commit.changed
            )
            assert (
                await runtime.app.call(READ_DRAFT, ReadDraft(draft=invalid.draft))
            ).body == answer

            named = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="alpha\nskip\nomega"), title="Named source", after=policy
                ),
            )
            named_view = await runtime.app.call(
                READ_RECORD, ReadRecord(target=named.commit.revision_ref)
            )
            selections = (
                ContentSelection(
                    artifact=named_view.body_artifact,
                    selection=SourceSelection(start_byte=0, end_byte=5),
                ),
                ContentSelection(
                    artifact=named_view.body_artifact,
                    selection=SourceSelection(start_byte=11, end_byte=16),
                ),
            )
            answer = "# Saved\n\nDistinctive completed memory."
            saved_writing = await runtime.app.call(
                WRITE_DRAFT,
                NewDraftRequest(
                    sources=(
                        SourceChoice(
                            target=RevisionSelection(
                                revision=named.commit.revision_ref, selections=selections
                            )
                        ),
                    ),
                    adoption="save",
                    after=policy,
                ),
            )
            assert saved_writing.record is not None
            sent = json.dumps(requests[-1], ensure_ascii=False)
            assert "Named source" in sent and "alpha" in sent and "omega" in sent
            stored = await runtime.app.call(
                READ_RECORD,
                ReadRecord(target=saved_writing.record.commit.revision_ref, sources=True),
            )
            assert (
                tuple(item for binding in stored.sources for item in binding.selections)
                == selections
            )
            result = await runtime.app.call(
                QUERY, CognitiveQuery(text="completed memory", mode="lexical")
            )
            assert any(group.context.revision_ref == stored.revision_ref for group in result.groups)

            answer = json.dumps(
                {
                    "edits": [
                        {
                            "target": "T1",
                            "action": "replace",
                            "before": "missing quotation",
                            "after": "new",
                        }
                    ]
                }
            )
            with pytest.raises(NousError) as failed:
                await runtime.app.call(
                    WRITE_DRAFT,
                    EditDraftRequest(
                        base=stored.revision_ref,
                        targets=(stored.body_selection,),
                        instruction="Edit the text",
                    ),
                )
            assert failed.value.code == "content.edit_quote_missing"
            product = await runtime.results.get(
                ResultRef.model_validate(failed.value.details["product"])
            )
            output = next(item.artifact for item in product.outputs if item.name == "output")
            assert json.loads(await runtime.artifacts.read_text(output)) == json.loads(answer)
            calls = len(requests)
            with pytest.raises(NousError) as retried:
                await runtime.app.call(DRAFT_FROM_RESULT, DraftFromResult(result=product.ref))
            assert retried.value.code == "content.edit_quote_missing" and len(requests) == calls
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=stored.revision_ref))
            ).body == stored.body

    with model_server(Handler) as server:
        asyncio.run(scenario())
