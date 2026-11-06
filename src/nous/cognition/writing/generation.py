"""Four explicit writing intents over frozen sources and one durable draft/adoption path."""

from __future__ import annotations

import difflib
import json
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from typing import Literal
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.operations import add_error_context
from nous.content.citations import normalize_source_links, parse_citations
from nous.content.contracts import (
    ContentSelection,
    InputGroup,
    InputPart,
    InputSpec,
    OutputContent,
    SourceSelection,
)
from nous.content.edits import ReplaceSelection, prepare_text_edit
from nous.content.text import BlockRef
from nous.models.operations import GenerationOperations, ProducedTask
from nous.temporal.values import CognitiveTimes

from ..contracts import NamespaceRef, RecordRevisionRef, RecordView, SourceBinding
from ..sources import PreparedSources, SourceInput
from .contracts import (
    DEFAULT_AFTER_WRITE,
    AfterWrite,
    CitationContext,
    DraftContext,
    DraftRef,
    DraftView,
    NormalizedEdit,
    SourceChanges,
    WritingOutcome,
)
from .drafts import DraftStore
from .operations import RecordOperations
from .reading import RecordReader
from .records import WrittenRecords


@dataclass(frozen=True)
class PendingWriting:
    produced: ProducedTask
    context: DraftContext
    adoption: Literal["draft", "save"]
    after: AfterWrite
    base: RecordView | None
    sources: PreparedSources


class WritingOperations:
    """Generate writing from frozen sources for one bound namespace.

    Each intent saves a successful model result and draft before optional formal
    adoption. ``adoption="draft"`` leaves the draft for inspection; ``"save"``
    then adopts it. The returned outcome identifies the draft and any committed
    record. A failed adoption leaves the saved draft available for a separate retry.
    """

    def __init__(
        self,
        namespace: NamespaceRef,
        core: WrittenRecords,
        reader: RecordReader,
        generation: GenerationOperations,
        drafts: DraftStore,
        records: RecordOperations,
    ):
        self.namespace, self.core, self.reader = namespace, core, reader
        self.generation, self.drafts, self.records = generation, drafts, records
        self.execution, self.database, self.results = (
            reader.execution,
            reader.database,
            generation.results,
        )

    async def _freeze(
        self,
        sources: Sequence[SourceInput],
        *,
        prefix: str = "S",
        reserved: Sequence[str] = (),
        reject_journals: bool = False,
    ) -> PreparedSources:
        return await self.core.sources.prepare(
            self.namespace,
            sources,
            prefix=prefix,
            reserved_keys=reserved,
            reject_journals=reject_journals,
        )

    @staticmethod
    def _inputs(sources: PreparedSources) -> list[InputSpec]:
        return [
            InputSpec(
                kind="artifact",
                artifact=item.artifact,
                selection=item.selection,
                slot_key=source.key,
                role=source.usage,
            )
            for source in sources.bindings
            for item in sources.actual[source.key]
        ]

    async def _attach_origins(
        self, produced: ProducedTask, prepared: PreparedSources
    ) -> ProducedTask:
        """Keep consumed representations while attaching their exact formal origins."""
        bindings = {item.key: item for item in prepared.bindings}
        groups: list[InputGroup] = []
        for group in produced.actual_inputs:
            source = bindings.get(group.slot_key)
            if source is None or source.owner_revision is None:
                groups.append(group)
                continue
            target = source.selections[0].artifact
            representation_ids = {item.artifact.id for item in prepared.actual[source.key]}
            parts: list[InputPart] = []
            for part in group.parts:
                origins = list(part.origins)
                candidates = (part.content, *(item.origin for item in part.origins))
                for item in candidates:
                    if item.artifact.id in representation_ids and item.artifact.id != target.id:
                        for origin in await self.results.artifacts.map_origins(item, target):
                            if origin not in origins:
                                origins.append(origin)
                parts.append(part.model_copy(update={"origins": tuple(origins)}))
            groups.append(group.model_copy(update={"parts": tuple(parts)}))
        return replace(produced, actual_inputs=tuple(groups))

    @staticmethod
    def _actual_bindings(
        produced: ProducedTask, selected: Sequence[SourceBinding]
    ) -> tuple[SourceBinding, ...]:
        source_map = {source.key: source for source in selected}
        output: list[SourceBinding] = []
        for group in produced.actual_inputs:
            if group.role not in {"source", "reference", "current_basis", "inspiration"}:
                continue
            source = source_map.get(group.slot_key)
            if source is None:
                raise NousError(
                    "cognition.source_mismatch",
                    "Successful input has no frozen source",
                    details={"key": group.key},
                )
            ids = {item.artifact.id for item in source.selections}
            coordinates = tuple(
                dict.fromkeys(
                    content
                    for part in group.parts
                    for content in (part.content, *(origin.origin for origin in part.origins))
                    if content.artifact.id in ids
                )
            )
            if not coordinates:
                raise NousError(
                    "cognition.source_mismatch",
                    "Actual source group has no exact original selections",
                    details={"key": group.key},
                )
            output.append(source.model_copy(update={"key": group.key, "selections": coordinates}))
        return tuple(output)

    async def _base(self, base: str | RecordRevisionRef, *, journal_only: bool) -> RecordView:
        if isinstance(base, str) and "@" not in base:
            raise NousError("input.expected_required", "An exact revision with @N is required")
        view = await self.reader.read(base, namespace=self.namespace.id)
        if journal_only and view.ref.kind != "journal":
            raise NousError("input.invalid_kind", "This writing intent requires a Journal")
        return view

    async def _feedback(
        self, base: RecordRevisionRef, refs: Sequence[str], mappings: Mapping[str, str]
    ) -> str:
        rendered: list[str] = []
        for opinion in await self.records.feedback.select_for_edit(refs, base, mappings=mappings):
            rendered.append(
                f"[{opinion.identifier}] {opinion.category}: {opinion.value or ''}\n{opinion.comment or ''}"
            )
        if mappings:
            rendered.append(
                "Explicit mappings to this base:\n" + json.dumps(dict(mappings), ensure_ascii=False)
            )
        return "\n\n".join(rendered)

    async def _produce(
        self,
        task: str,
        contract: str,
        inputs: Sequence[InputSpec],
        *,
        question: str,
        instructions: str,
        on_delta: Callable[[str], None] | None = None,
    ) -> ProducedTask:
        snapshot = self.execution.snapshot
        frozen = self.generation.executor.freeze(task, snapshot)
        if frozen.resource is None or frozen.resource.manifest.contract_id != contract:
            raise NousError(
                "model.task_contract_mismatch",
                "Writing task uses another output contract",
                details={"task": task, "expected_contract": contract},
            )
        prepared = await self.generation.prepare_input(
            frozen, inputs, snapshot, question=question, instructions=instructions
        )
        if len(prepared.batches) != 1:
            raise NousError(
                "cognition.joint_required",
                "Select one topic and fewer sources for one writing operation",
            )
        return await self.generation.produce(
            prepared.frozen,
            prepared.batches[0],
            snapshot,
            question=question,
            instructions=instructions,
            completed=list(prepared.preparations),
            on_delta=on_delta,
            producer_key=f"writing:{uuid4()}",
        )

    async def _complete(
        self,
        produced: ProducedTask,
        context: DraftContext,
        adoption: Literal["draft", "save"],
        after: AfterWrite,
        base: RecordView | None = None,
        *,
        sources: PreparedSources,
    ) -> WritingOutcome:
        try:
            return await self._complete_inner(produced, context, adoption, after, base, sources)
        except BaseException as error:
            add_error_context(
                error,
                {
                    "stage": "writing.complete",
                    "model_succeeded": True,
                    "completed_result_refs": list(produced.completed_stage_refs),
                },
                recovery=PendingWriting(produced, context, adoption, after, base, sources),
            )
            raise

    async def complete_pending(self, pending: PendingWriting) -> WritingOutcome:
        """Continue a successful generation from normalization without another request."""
        return await self._complete(
            pending.produced,
            pending.context,
            pending.adoption,
            pending.after,
            pending.base,
            sources=pending.sources,
        )

    async def _complete_inner(
        self,
        produced: ProducedTask,
        context: DraftContext,
        adoption: Literal["draft", "save"],
        after: AfterWrite,
        base: RecordView | None,
        sources: PreparedSources,
    ) -> WritingOutcome:
        produced = await self._attach_origins(produced, sources)
        actual = self._actual_bindings(produced, context.selected_sources)
        produced = await self._normalize_links(produced, context, actual, base)
        text = produced.outputs["output"].data.decode("utf-8")
        inherited = context.citation_context.inherited_sources
        if base is not None:
            used = {citation.key for citation in parse_citations(text).citations}
            additions = tuple(
                source for source in actual if source.key in used or source.usage == "current_basis"
            )
            bindings = SourceChanges(add=additions).apply_to(inherited)
        else:
            bindings = actual
        context = context.model_copy(update={"source_bindings": tuple(bindings)})
        outcome = await self.drafts.save_produced(produced, context)
        if outcome.status == "no_material":
            return outcome
        if adoption == "draft":
            return outcome
        assert outcome.draft is not None
        adopted = await self.records.adopt(outcome.draft.ref, after=after)
        draft = outcome.draft.model_copy(
            update={"adopted_revision": adopted.record.revision_ref, "adopted_as": "generated"}
        )
        return WritingOutcome(
            status="adopted" if adopted.record.changed else "unchanged",
            draft_ref=draft.ref,
            draft=draft,
            record=adopted,
            stage_results=outcome.stage_results,
        )

    async def _normalize_links(
        self,
        produced: ProducedTask,
        context: DraftContext,
        actual: Sequence[SourceBinding],
        base: RecordView | None,
    ) -> ProducedTask:
        """Interpret known source destinations while retaining the actual model response."""
        keys = tuple(
            source.key for source in (*context.citation_context.inherited_sources, *actual)
        )
        outputs = dict(produced.outputs)
        repairs: list[JsonValue] = []
        program_outputs = TypeAdapter(list[str]).validate_python(
            produced.parameters.get("program_outputs", [])
        )
        if context.intent == "edit":
            assert base is not None
            edits = TypeAdapter(tuple[NormalizedEdit, ...]).validate_json(outputs["edits"].data)
            changes: list[ReplaceSelection] = []
            for edit in edits:
                normalized = normalize_source_links(edit.after, keys, preserve=edit.before)
                repairs.extend(normalized.keys)
                changes.append(
                    ReplaceSelection(
                        selection=ContentSelection(
                            artifact=base.body_artifact,
                            selection=SourceSelection(
                                start_byte=edit.start_byte, end_byte=edit.end_byte
                            ),
                        ),
                        expected_text=edit.before,
                        replacement=normalized.text,
                    )
                )
            if not repairs:
                return produced
            view = await self.results.artifacts.text_view(base.body_artifact)
            edited = prepare_text_edit(view, changes)
            outputs["output"] = OutputContent(data=edited.utf8_bytes)
            outputs["edits"] = OutputContent(
                data=TypeAdapter(tuple[NormalizedEdit, ...]).dump_json(
                    tuple(
                        NormalizedEdit(
                            start_byte=change.start_byte,
                            end_byte=change.end_byte,
                            before=change.expected_bytes.decode("utf-8"),
                            after=change.replacement_bytes.decode("utf-8"),
                        )
                        for change in edited.changes
                    )
                ),
                media_type="application/json",
                display_name="edits.json",
            )
            diff = edited.diff
        else:
            original = outputs["output"]
            normalized = normalize_source_links(
                original.data.decode("utf-8"), keys, preserve=base.body if base else None
            )
            if not normalized.keys:
                return produced
            repairs.extend(normalized.keys)
            outputs["model_output"] = original
            outputs["output"] = original.model_copy(
                update={"data": normalized.text.encode("utf-8")}
            )
            program_outputs.append("output")
            diff = (
                "".join(
                    difflib.unified_diff(
                        base.body.splitlines(keepends=True),
                        normalized.text.splitlines(keepends=True),
                        fromfile="before",
                        tofile="after",
                    )
                )
                if base
                else None
            )
        if diff is not None:
            outputs["diff"] = OutputContent(
                data=diff.encode("utf-8"), media_type="text/plain", display_name="changes.diff"
            )
            program_outputs.append("diff")
        return replace(
            produced,
            outputs=outputs,
            parameters={
                **produced.parameters,
                "program_outputs": list(dict.fromkeys(program_outputs)),
                "source_link_normalizations": repairs,
            },
        )

    @staticmethod
    def _policy(adoption: Literal["draft", "save"], after: AfterWrite) -> None:
        if adoption != "save" and (
            after.retrieval == "refresh" or after.retrieval_mode is not None
        ):
            raise NousError(
                "input.refresh_requires_save", "Use save adoption when requesting refresh"
            )

    async def journalize(
        self,
        sources: Sequence[SourceInput],
        *,
        topic: str | None = None,
        purpose: str = "record",
        viewpoint: str | None = None,
        author: str | None = None,
        times: CognitiveTimes | None = None,
        adoption: Literal["draft", "save"] | None = None,
        on_delta: Callable[[str], None] | None = None,
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> WritingOutcome:
        """Generate a new Journal draft from selected non-Journal sources.

        Sources are frozen before the model request. Omitted adoption uses configuration.
        ``on_delta`` receives streamed model text when available.

        Returns:
            A writing outcome containing the saved draft and, with save adoption, the
            committed record. ``no_material`` indicates no sources or a model result
        signaling no material.

        Raises:
            NousError: ``input.refresh_requires_save`` if refresh is requested without
                save adoption; ``cognition.writing_intent_required`` if a source is a
                Journal body; ``cognition.joint_required`` if inputs require multiple
                batches; or ``model.task_contract_mismatch`` for the wrong task contract.
        """
        async with self.execution.operation("journalize"):
            config = self.execution.snapshot.values
            policy = adoption or config.cognition.journalize.adoption
            self._policy(policy, after)
            if times is not None:
                await self.core.time.validate([times])
            prepared = await self._freeze(sources, reject_journals=True)
            ns, selected = prepared.namespace, prepared.bindings
            if not selected:
                return WritingOutcome(status="no_material")
            task = config.cognition.journalize.task
            produced = await self._produce(
                task,
                "journal-draft",
                self._inputs(prepared),
                question=topic or "",
                instructions=f"Purpose: {purpose}\nAuthor: {author or ''}\nViewpoint: {viewpoint or ''}",
                on_delta=on_delta,
            )
            context = DraftContext(
                namespace=ns,
                intent="new",
                selected_sources=selected,
                citation_context=CitationContext(new_sources=selected),
                topic=topic,
                purpose=purpose,
                author=author,
                viewpoint=viewpoint,
                times=await self.core.prepare_times(times),
                config_version=self.execution.snapshot.version,
                task=task,
            )
            return await self._complete(produced, context, policy, after, sources=prepared)

    async def propose_edit(
        self,
        base: str | RecordRevisionRef,
        instruction: str,
        *,
        scopes: Sequence[BlockRef | ContentSelection] | None = None,
        blocks: Sequence[str] = (),
        sources: Sequence[SourceInput] = (),
        feedback_refs: Sequence[str] = (),
        feedback_mappings: Mapping[str, str] | None = None,
        intent: Literal["edit", "correction"] = "edit",
        adoption: Literal["draft", "save"] = "draft",
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> WritingOutcome:
        """Generate a scoped edit of an exact record revision.

        Supply block keys or exact scopes, not both; omitting scopes selects top-level
        blocks. Feedback on another revision of the same record needs a mapping note.
        The result is saved as a draft before optional adoption.

        Returns:
            A writing outcome containing the saved draft and, with save adoption, the
            committed record. ``unchanged`` identifies unchanged output;
            ``no_material`` indicates no usable output.

        Raises:
            NousError: ``input.expected_required`` if a string base lacks ``@N``;
                ``input.instruction_required`` without instruction or feedback;
                ``input.conflicting_scope`` for both scope forms;
                ``content.edit_scope_invalid`` for an empty or invalid scope;
                ``input.refresh_requires_save`` for refresh without save adoption; or
                ``cognition.revision_conflict`` if the base changes before adoption.
        """
        async with self.execution.operation("writing.propose_edit"):
            self._policy(adoption, after)
            original = await self._base(base, journal_only=False)
            feedback = await self._feedback(
                original.revision_ref, feedback_refs, feedback_mappings or {}
            )
            if not instruction.strip() and not feedback:
                raise NousError(
                    "input.instruction_required", "Provide an edit instruction or exact feedback"
                )
            view = await self.results.artifacts.text_view(original.body_selection)
            if blocks and scopes is not None:
                raise NousError("input.conflicting_scope", "Choose block keys or exact scopes")
            chosen = (
                [block.selection for block in view.blocks if block.parent_key is None]
                if scopes is None
                else []
            )
            if blocks:
                chosen = [view.block(key).selection for key in blocks]
            for scope in scopes or ():
                if isinstance(scope, BlockRef):
                    if view.block(scope.key).ref != scope:
                        raise NousError(
                            "content.edit_scope_invalid", "Block belongs to another exact text view"
                        )
                    content = scope.content
                else:
                    content = scope
                view.selection_bounds(content)
                chosen.append(content)
            if not chosen:
                raise NousError("content.edit_scope_invalid", "Provide at least one editable range")
            targets = {f"T{index}": selected for index, selected in enumerate(chosen, 1)}
            bounds = [view.selection_bounds(selected) for selected in chosen]
            inputs = [
                InputSpec(
                    kind="artifact",
                    artifact=selected.artifact,
                    selection=selected.selection,
                    slot_key=key,
                    role="edit_target",
                )
                for key, selected in targets.items()
            ]
            for index, block in enumerate(view.blocks, 1):
                low, high = view.selection_bounds(block.selection)
                if not any(low < end and start < high for start, end in bounds):
                    inputs.append(
                        InputSpec(
                            kind="artifact",
                            artifact=block.selection.artifact,
                            selection=block.selection.selection,
                            slot_key=f"CTX{index}",
                            role="context",
                        )
                    )
            prepared = await self._freeze(
                sources, prefix="N", reserved=[source.key for source in original.sources]
            )
            ns, selected = prepared.namespace, prepared.bindings
            inputs.extend(self._inputs(prepared))
            task = self.execution.snapshot.values.cognition.edit.task
            instructions = (
                f"Editable target slots: {', '.join(targets)}. CTX sources are read-only.\n"
                f"Existing citation directory: {json_sources(original.sources)}\n"
                "Preserve existing source links; new source keys are the actual N groups shown below.\n"
                f"Feedback opinions (not external facts):\n{feedback}"
            )
            produced = await self._produce(
                task, "journal-edit", inputs, question=instruction, instructions=instructions
            )
            produced = replace(
                produced,
                actual_inputs=(
                    *produced.actual_inputs,
                    InputGroup(
                        key="BASE",
                        slot_key="BASE",
                        role="revision_base",
                        name=original.title or "Original body",
                        parts=(
                            InputPart(
                                content=original.body_selection,
                                representation="text",
                                media_type="text/markdown",
                            ),
                        ),
                    ),
                ),
                parameters={**produced.parameters, "program_input_keys": ["BASE"]},
            )
            context = DraftContext(
                namespace=ns,
                intent="edit",
                base_revision=original.revision_ref,
                selected_sources=selected,
                citation_context=CitationContext(
                    inherited_sources=original.sources, new_sources=selected, targets=targets
                ),
                feedback_refs=tuple(dict.fromkeys(feedback_refs)),
                feedback_mappings=dict(feedback_mappings or {}),
                change_kind=intent,
                instruction=instruction,
                purpose=original.purpose,
                author=original.author,
                viewpoint=original.viewpoint,
                times=original.times,
                config_version=self.execution.snapshot.version,
                task=task,
            )
            return await self._complete(
                produced, context, adoption, after, original, sources=prepared
            )

    async def rewrite(
        self,
        base: str | RecordRevisionRef,
        instruction: str,
        *,
        sources: Sequence[SourceInput] = (),
        intent: Literal["edit", "correction"] = "edit",
        adoption: Literal["draft", "save"] = "draft",
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> WritingOutcome:
        """Generate a full rewrite of an exact Journal revision.

        Additional sources are frozen before the request. The draft is saved before
        optional adoption, which checks that the base remains current.

        Returns:
            A writing outcome containing the saved draft and, with save adoption, the
            committed record. ``unchanged`` identifies unchanged output;
            ``no_material`` indicates no usable output.

        Raises:
            NousError: ``input.instruction_required`` for a blank instruction;
                ``input.expected_required`` if a string base lacks ``@N``;
                ``input.invalid_kind`` if the base is not a Journal;
                ``input.refresh_requires_save`` for refresh without save adoption; or
                ``cognition.revision_conflict`` if the base changes before adoption.
        """
        async with self.execution.operation("writing.rewrite"):
            self._policy(adoption, after)
            if not instruction.strip():
                raise NousError(
                    "input.instruction_required", "Provide an explicit rewrite instruction"
                )
            original = await self._base(base, journal_only=True)
            prepared = await self._freeze(
                sources, prefix="N", reserved=[source.key for source in original.sources]
            )
            ns, selected = prepared.namespace, prepared.bindings
            inputs = [
                InputSpec(
                    kind="artifact",
                    artifact=original.body_artifact,
                    slot_key="BASE",
                    role="revision_base",
                ),
                *self._inputs(prepared),
            ]
            task = self.execution.snapshot.values.cognition.rewrite.task
            produced = await self._produce(
                task,
                "journal-rewrite",
                inputs,
                question=instruction,
                instructions=f"Existing citation directory: {json_sources(original.sources)}\nBASE is the complete frozen draft. Preserve its source keys; new N keys are supplied separately.",
            )
            context = DraftContext(
                namespace=ns,
                intent="rewrite",
                base_revision=original.revision_ref,
                selected_sources=selected,
                citation_context=CitationContext(
                    inherited_sources=original.sources, new_sources=selected
                ),
                instruction=instruction,
                change_kind=intent,
                purpose=original.purpose,
                author=original.author,
                viewpoint=original.viewpoint,
                times=original.times,
                config_version=self.execution.snapshot.version,
                task=task,
            )
            return await self._complete(
                produced, context, adoption, after, original, sources=prepared
            )

    async def reflect(
        self,
        journals: Sequence[str | RecordRevisionRef],
        topic: str,
        *,
        sources: Sequence[SourceInput] = (),
        author: str | None = None,
        viewpoint: str | None = None,
        adoption: Literal["draft", "save"] = "draft",
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> WritingOutcome:
        """Generate a new Journal about a topic from exact Journal revisions.

        Additional sources are frozen before generation; the draft is saved before
        optional adoption.

        Returns:
            A writing outcome containing the saved draft and, with save adoption, the
            committed record. ``no_material`` indicates a model result signaling no
            material.

        Raises:
            NousError: ``input.reflection_required`` without Journals or a nonblank
                topic; ``input.expected_required`` if a string revision lacks ``@N``;
                ``input.invalid_kind`` for a non-Journal source revision; or
                ``input.refresh_requires_save`` for refresh without save adoption.
        """
        async with self.execution.operation("writing.reflect"):
            self._policy(adoption, after)
            if not journals or not topic.strip():
                raise NousError(
                    "input.reflection_required",
                    "Reflection requires exact Journals and a new topic",
                )
            originals = [await self._base(target, journal_only=True) for target in journals]
            first = await self._freeze([view.revision_ref for view in originals], prefix="J")
            additional = await self._freeze(sources, prefix="N")
            ns = first.namespace
            prepared = PreparedSources(
                ns, (*first.bindings, *additional.bindings), {**first.actual, **additional.actual}
            )
            selected = prepared.bindings
            task = self.execution.snapshot.values.cognition.reflection.task
            produced = await self._produce(
                task,
                "journal-reflect",
                self._inputs(prepared),
                question=topic,
                instructions=f"Author: {author or ''}\nViewpoint: {viewpoint or ''}\n"
                "Write a new Journal. Source documents and their internal source: directories remain in their own namespaces; cite only actual J/N groups provided here.",
            )
            context = DraftContext(
                namespace=ns,
                intent="reflection",
                selected_sources=selected,
                citation_context=CitationContext(new_sources=selected),
                topic=topic,
                purpose="reflection",
                author=author,
                viewpoint=viewpoint,
                times=await self.core.prepare_times(None),
                config_version=self.execution.snapshot.version,
                task=task,
            )
            return await self._complete(produced, context, adoption, after, sources=prepared)

    async def read_draft(self, target: str | DraftRef, *, sources: bool = False) -> DraftView:
        """Read a saved draft in the bound namespace.

        Set ``sources`` to load its source content views as well as frozen bindings.
        """
        return await self.drafts.read(target, namespace=self.namespace.id, sources=sources)


def json_sources(sources: Sequence[SourceBinding]) -> str:
    return json.dumps(
        [
            {"key": source.key, "name": source.name or source.key, "usage": source.usage}
            for source in sources
        ],
        ensure_ascii=False,
    )
