"""Import materials and run repeatable retrieval experiments through the Nous SDK."""

from __future__ import annotations

import argparse
import ast
import asyncio
import hashlib
import json
import re
import tomllib
from pathlib import Path
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue, TypeAdapter

from nous.application.inputs import FileInput
from nous.cognition.annotation.contracts import AnnotateRecord
from nous.cognition.annotation.operations import ANNOTATE_RECORD
from nous.cognition.contracts import RecordRevisionRef
from nous.cognition.retrieval.contracts import CognitiveQuery, QueryResult
from nous.cognition.retrieval.operations import QUERY
from nous.cognition.writing.contracts import (
    AddNote,
    AdoptDraft,
    AfterWrite,
    NewDraftRequest,
    ReadDraft,
    ReplaceRecord,
    ReviseDraft,
    SourceChoice,
)
from nous.cognition.writing.generation import READ_DRAFT, REVISE_DRAFT, WRITE_DRAFT
from nous.cognition.writing.operations import ADD_NOTE, ADOPT_DRAFT, REPLACE_RECORD
from nous.common.errors import NousError
from nous.content.citations import parse_citations
from nous.content.contracts import ExternalInput, InlineText, ReadResult, SourceSelection
from nous.content.operations import READ_RESULT
from nous.runtime.bootstrap import open_instance

AFTER = AfterWrite(annotation="skip")


class Corpus(BaseModel):
    model_config = ConfigDict(extra="forbid")
    text: list[str] = Field(default_factory=list)
    docstrings: list[str] = Field(default_factory=list)


class InputSource(BaseModel):
    model_config = ConfigDict(extra="forbid")
    path: str
    selection: SourceSelection | None = None
    usage: Literal["reference", "current_basis", "inspiration"] = "reference"


class DraftCase(BaseModel):
    model_config = ConfigDict(extra="forbid")
    name: str
    topic: str | None = None
    sources: list[InputSource]


class QueryCase(BaseModel):
    model_config = ConfigDict(extra="forbid")
    question: str
    modes: list[Literal["auto", "lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"]] = [
        "hybrid"
    ]
    top_k: int = 8


class ExperimentPlan(BaseModel):
    model_config = ConfigDict(extra="forbid")
    corpus: Corpus = Field(default_factory=Corpus)
    draft: list[DraftCase] = Field(default_factory=list)
    query: list[QueryCase] = Field(default_factory=list)


class ImportedNote(BaseModel):
    digest: str
    ref: RecordRevisionRef
    literal_markdown: bool = False


class SavedDraft(BaseModel):
    name: str
    ref: str
    body: str
    sources: list[dict[str, JsonValue]]
    producer: dict[str, JsonValue]
    inputs: list[dict[str, JsonValue]]


class AdoptedDraft(BaseModel):
    draft: str
    record: str


class ExperimentState(BaseModel):
    instance_id: str
    namespace: str
    notes: dict[str, ImportedNote] = Field(default_factory=dict)
    drafts: list[SavedDraft] = Field(default_factory=list)
    adopted: list[AdoptedDraft] = Field(default_factory=list)


class SearchResult(BaseModel):
    question: str
    mode: Literal["auto", "lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"]
    top_k: int = 8
    result: QueryResult | None = None
    error: str | None = None


def docstrings(path: Path) -> str:
    """Collect public Python documentation while retaining the source file as evidence."""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    nodes: list[tuple[str, ast.Module | ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef]] = [
        (path.stem, tree)
    ]
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if node.name.startswith("_"):
                continue
            nodes.append((node.name, node))
            if isinstance(node, ast.ClassDef):
                nodes.extend(
                    (f"{node.name}.{method.name}", method)
                    for method in node.body
                    if isinstance(method, (ast.FunctionDef, ast.AsyncFunctionDef))
                    and not method.name.startswith("_")
                )
    parts = [(name, ast.get_docstring(node)) for name, node in nodes]
    return "\n\n".join(f"## {name}\n\n{value}" for name, value in parts if value)


def save_json(path: Path, data: object) -> None:
    """Replace an output after each completed operation so interrupted runs can resume."""
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    temporary.replace(path)


def describe_error(error: Exception) -> str:
    """Expose the operation and its underlying error without dumping large diagnostics."""
    if not isinstance(error, NousError):
        return str(error)
    pieces = [f"{error.code}: {error}"]
    cause = error.__cause__
    if isinstance(cause, NousError):
        pieces.append(f"{cause.code}: {cause}")
        status = cause.details.get("status_code")
        if status is not None:
            pieces.append(f"HTTP {status}")
    return "; ".join(pieces)


def rate_limited(error: Exception) -> bool:
    """Recognize an actual provider rate limit in a query or its build failure."""
    current: BaseException | None = error
    while current is not None:
        if isinstance(current, NousError) and current.details.get("status_code") == 429:
            return True
        current = current.__cause__
    return False


def write_drafts(output: Path, drafts: list[SavedDraft]) -> None:
    sections = ["# 待审阅草稿\n"]
    for draft in drafts:
        sections.append(f"## {draft.name}\n\n草稿：`{draft.ref}`\n\n{draft.body}\n")
    (output / "drafts.md").write_text("\n".join(sections), encoding="utf-8")


def write_queries(output: Path, results: list[SearchResult]) -> None:
    sections = ["# 检索结果\n"]
    for entry in results:
        sections.append(f"## {entry.question} · {entry.mode}\n")
        if entry.result is None:
            sections.append(f"查询失败：{entry.error}\n")
            continue
        for group in entry.result.groups:
            passage = group.passages[0]
            sections.append(
                f"{group.rank}. **{group.title or '未命名'}** "
                f"`{group.context.revision_ref.identifier}`\n\n"
                f"   {passage.delivery.text}\n"
            )
    (output / "queries.md").write_text("\n".join(sections), encoding="utf-8")


def selected_files(root: Path, patterns: list[str]) -> list[Path]:
    """Expand project-relative patterns in a stable order without duplicate imports."""
    return sorted({path for pattern in patterns for path in root.glob(pattern) if path.is_file()})


async def run(args: argparse.Namespace) -> None:
    root = Path.cwd().resolve()
    plan = ExperimentPlan.model_validate(tomllib.loads(args.plan.read_text(encoding="utf-8")))
    args.output.mkdir(parents=True, exist_ok=True)
    state_path = args.output / "state.json"
    state = (
        ExperimentState.model_validate_json(state_path.read_text(encoding="utf-8"))
        if state_path.exists()
        else None
    )
    async with open_instance(args.instance) as runtime:
        instance_id = runtime.descriptor.instance_id
        if state and (state.instance_id != instance_id or state.namespace != args.namespace):
            raise ValueError("输出目录属于另一个实例或命名空间，请选择新的输出目录。")
        if not state:
            state = ExperimentState(instance_id=instance_id, namespace=args.namespace)
            save_json(state_path, state.model_dump(mode="json"))
        if args.command == "ingest":
            groups = [
                ("text", selected_files(root, plan.corpus.text)),
                ("docstrings", selected_files(root, plan.corpus.docstrings)),
            ]
            failures = 0
            for kind, paths in groups:
                for path in paths:
                    name = path.relative_to(root).as_posix()
                    try:
                        body = (
                            docstrings(path)
                            if kind == "docstrings"
                            else path.read_text(encoding="utf-8")
                        )
                        if not body.strip():
                            continue
                        digest = hashlib.sha256(path.read_bytes()).hexdigest()
                        previous = state.notes.get(name)
                        if previous and previous.digest == digest:
                            continue
                        literal = bool(parse_citations(body).citations)
                        if literal:
                            # Existing source links in imported material are literal text.
                            fence = "`" * max(
                                4, max((len(run) for run in re.findall(r"`+", body)), default=0) + 1
                            )
                            body = f"{fence}markdown\n{body}\n{fence}\n"
                        if previous:
                            outcome = await runtime.app.call(
                                REPLACE_RECORD,
                                ReplaceRecord(
                                    target=previous.ref,
                                    namespace=args.namespace,
                                    body=InlineText(text=body),
                                    sources=(SourceChoice(target=ExternalInput(key="original")),),
                                    after=AFTER,
                                ),
                                inputs={"original": FileInput(path)},
                            )
                        else:
                            outcome = await runtime.app.call(
                                ADD_NOTE,
                                AddNote(
                                    namespace=args.namespace,
                                    body=InlineText(text=body),
                                    remember=True,
                                    title=name,
                                    sources=(SourceChoice(target=ExternalInput(key="original")),),
                                    after=AFTER,
                                ),
                                inputs={"original": FileInput(path)},
                            )
                        state.notes[name] = ImportedNote(
                            digest=digest, ref=outcome.commit.revision_ref, literal_markdown=literal
                        )
                        save_json(state_path, state.model_dump(mode="json"))
                        print(f"{'已修订' if previous else '已导入'} {name}", flush=True)
                    except Exception as error:
                        failures += 1
                        print(f"导入失败 {name}: {describe_error(error)}", flush=True)
            print(f"已保存 {len(state.notes)} 条 Note，失败 {failures} 条。")
            if failures:
                raise SystemExit(1)

        elif args.command == "draft":
            cases = plan.draft
            if args.names:
                cases = [case for case in cases if case.name in args.names]
                missing = set(args.names) - {case.name for case in cases}
                if missing:
                    raise ValueError(f"找不到草稿任务：{', '.join(sorted(missing))}")
            failures = 0
            for case in cases:
                try:
                    requests: list[SourceChoice] = []
                    inputs: dict[str, FileInput] = {}
                    for index, source in enumerate(case.sources):
                        name = source.path
                        note = state.notes.get(name)
                        if note and source.selection is None:
                            target = note.ref
                        else:
                            key = f"source_{index}"
                            inputs[key] = FileInput(root / name)
                            target = ExternalInput(
                                key=key, selection=source.selection or SourceSelection()
                            )
                        requests.append(SourceChoice(target=target, usage=source.usage))
                    outcome = await runtime.app.call(
                        WRITE_DRAFT,
                        NewDraftRequest(
                            namespace=args.namespace, sources=tuple(requests), topic=case.topic
                        ),
                        inputs=inputs,
                    )
                    if outcome.draft is None:
                        print(f"无草稿 {case.name}: {outcome.status}", flush=True)
                        continue
                    draft = await runtime.app.call(READ_DRAFT, ReadDraft(draft=outcome.draft))
                    result = await runtime.app.call(READ_RESULT, ReadResult(ref=draft.result_ref))
                    state.drafts.append(
                        SavedDraft(
                            name=case.name,
                            ref=draft.ref.identifier,
                            body=draft.body,
                            sources=[
                                binding.model_dump(mode="json")
                                for binding in draft.context.source_bindings
                            ],
                            producer=result.producer.model_dump(mode="json"),
                            inputs=[group.model_dump(mode="json") for group in result.inputs],
                        )
                    )
                    save_json(state_path, state.model_dump(mode="json"))
                    write_drafts(args.output, state.drafts)
                    print(f"已生成 {case.name}: {draft.ref.identifier}", flush=True)
                except Exception as error:
                    failures += 1
                    print(f"生成失败 {case.name}: {describe_error(error)}", flush=True)
            if failures:
                raise SystemExit(1)

        elif args.command == "adopt":
            draft_ref = args.ref
            if args.body:
                revised = await runtime.app.call(
                    REVISE_DRAFT,
                    ReviseDraft(
                        draft=draft_ref,
                        namespace=args.namespace,
                        markdown=ExternalInput(key="body"),
                    ),
                    inputs={"body": FileInput(args.body)},
                )
                draft_ref = revised.ref.identifier
            outcome = await runtime.app.call(
                ADOPT_DRAFT,
                AdoptDraft(
                    draft=draft_ref, namespace=args.namespace, title=args.title, after=AFTER
                ),
            )
            identifier = outcome.commit.revision_ref.identifier
            state.adopted.append(AdoptedDraft(draft=draft_ref, record=identifier))
            save_json(state_path, state.model_dump(mode="json"))
            print(f"已采用：{identifier}")
            if args.annotate:
                annotation = await runtime.app.call(
                    ANNOTATE_RECORD, AnnotateRecord(target=identifier, namespace=args.namespace)
                )
                print(f"已采用标注：{annotation.model_dump_json()}")

        elif args.command == "query":
            path = args.output / "queries.json"
            results: list[SearchResult]
            pending: list[tuple[int, SearchResult]]

            def selected(entry: SearchResult) -> bool:
                return (
                    not args.contains
                    or any(text.casefold() in entry.question.casefold() for text in args.contains)
                ) and (not args.mode or entry.mode in args.mode)

            if args.retry_failed:
                results = TypeAdapter(list[SearchResult]).validate_json(
                    path.read_text(encoding="utf-8")
                )
                pending = [
                    (index, entry)
                    for index, entry in enumerate(results)
                    if entry.error and selected(entry)
                ]
            else:
                results = []
                pending = []
                for case in plan.query:
                    for mode in case.modes:
                        entry = SearchResult(
                            question=case.question, mode=mode, top_k=case.top_k, error="尚未执行"
                        )
                        if not selected(entry):
                            continue
                        pending.append((len(results), entry))
                        results.append(entry)
                if not results:
                    raise ValueError("没有与筛选条件匹配的查询。")
                save_json(path, [result.model_dump(mode="json") for result in results])
                write_queries(args.output, results)
            failures = 0
            limited = False
            for index, entry in pending:
                if limited and entry.mode != "lexical":
                    results[index] = entry.model_copy(
                        update={"result": None, "error": "上游限流，本次未执行"}
                    )
                    failures += 1
                    save_json(path, [result.model_dump(mode="json") for result in results])
                    write_queries(args.output, results)
                    continue
                try:
                    answer = await runtime.app.call(
                        QUERY,
                        CognitiveQuery(
                            namespace=args.namespace,
                            text=entry.question,
                            mode=entry.mode,
                            top_k=entry.top_k,
                        ),
                    )
                    results[index] = entry.model_copy(update={"result": answer, "error": None})
                    print(f"查询完成 {entry.mode}: {entry.question}", flush=True)
                except Exception as error:
                    failures += 1
                    limited = limited or rate_limited(error)
                    description = describe_error(error)
                    results[index] = entry.model_copy(update={"result": None, "error": description})
                    print(f"查询失败 {entry.mode}: {entry.question}: {description}", flush=True)
                save_json(path, [result.model_dump(mode="json") for result in results])
                write_queries(args.output, results)
            if failures:
                raise SystemExit(1)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--instance", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--namespace", default="default")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("ingest", help="Import configured text and Python documentation as Notes")
    draft = commands.add_parser("draft", help="Generate reviewable Journal drafts")
    draft.add_argument("names", nargs="*")
    adopt = commands.add_parser("adopt", help="Explicitly adopt a reviewed draft")
    adopt.add_argument("ref")
    adopt.add_argument("--title")
    adopt.add_argument("--body", type=Path, help="Reviewed replacement Markdown")
    adopt.add_argument("--annotate", action="store_true")
    query = commands.add_parser("query", help="Run configured questions and save full results")
    query.add_argument("--retry-failed", action="store_true", help="Retry only failed queries")
    query.add_argument(
        "--contains",
        action="append",
        help="Only questions containing this text; repeat for alternatives",
    )
    query.add_argument(
        "--mode",
        action="append",
        choices=("auto", "lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"),
        help="Only this mode from the plan; repeat for alternatives",
    )
    asyncio.run(run(parser.parse_args()))


if __name__ == "__main__":
    main()
