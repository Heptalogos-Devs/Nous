"""Complete writing operations over saved products, drafts and exact revisions."""

from __future__ import annotations

import difflib
import json
from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Literal

from pydantic import JsonValue, TypeAdapter

from nous.application.contracts import BasicEvents, CallContext, LanguageEvents, define_operation
from nous.common.errors import NousError
from nous.common.operations import OperationIssue, add_error_context
from nous.content.citations import normalize_source_links, parse_citations
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    ExternalInput,
    InputGroup,
    InputPart,
    OutputContent,
    OutputMapping,
    ProcessingResult,
    ProgramProducer,
    ResultFacts,
    ResultOutput,
    ResultOutputRef,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceRole,
    SourceSelection,
    UserProducer,
)
from nous.content.edits import PreparedTextEdit
from nous.content.selections import subtract_selection
from nous.models.generation import GenerationOperations, GenerationRequest

from ..contracts import NamespaceRef, RecordRevisionRef, RecordView, SourceBinding
from ..material_contracts import MaterialSelection, MaterialSource
from ..sources import PreparedSources
from ..work.refs import WorkRevisionRef
from .contracts import (
    AdoptDraft,
    CitationContext,
    DraftContext,
    DraftFromResult,
    DraftRef,
    DraftView,
    EditDraftRequest,
    NewDraftRequest,
    NormalizedEdit,
    ReadDraft,
    ReflectDraftRequest,
    ReviseDraft,
    RewriteDraftRequest,
    SourceChanges,
    SourceChoice,
    WriteDraft,
    WritingOutcome,
    WritingProductContext,
)
from .drafts import DraftStore
from .operations import RecordOperations
from .reading import RecordReader
from .records import WrittenRecords
from .tasks import EditProposal, compose_edit, writing_body

if TYPE_CHECKING:
    from ..materials import Materials


def _source_directory(sources: Sequence[SourceBinding]) -> str:
    return json.dumps(
        [
            {"key": source.key, "name": source.name or source.key, "usage": source.usage}
            for source in sources
        ],
        ensure_ascii=False,
    )


def _selection(artifact: ArtifactRef, start: int, end: int) -> ContentSelection:
    return ContentSelection(
        artifact=artifact, selection=SourceSelection(start_byte=start, end_byte=end)
    )


def _sequence_mappings(
    original: bytes, output: bytes, source: ArtifactRef, *, changed_is_derived: bool
) -> tuple[OutputMapping, ...]:
    """Preserve exact equal spans; changed text has only its declared semantic source."""
    mappings: list[OutputMapping] = []
    for tag, low, high, new_low, new_high in difflib.SequenceMatcher(
        None, original, output
    ).get_opcodes():
        if new_low == new_high:
            continue
        if tag == "equal":
            mappings.append(
                OutputMapping(
                    output_name="output",
                    selection=SourceSelection(start_byte=new_low, end_byte=new_high),
                    relation="identity",
                    origins=(_selection(source, low, high),),
                )
            )
        elif changed_is_derived:
            origin = (
                _selection(source, low, high) if low != high else ContentSelection(artifact=source)
            )
            mappings.append(
                OutputMapping(
                    output_name="output",
                    selection=SourceSelection(start_byte=new_low, end_byte=new_high),
                    relation="derived",
                    origins=(origin,),
                )
            )
    return tuple(mappings)


def _edit_mappings(
    base: bytes, edited: PreparedTextEdit, base_ref: ArtifactRef, model_ref: ArtifactRef
) -> tuple[OutputMapping, ...]:
    mappings: list[OutputMapping] = []
    old_offset = new_offset = 0
    for change in edited.changes:
        length = change.start_byte - old_offset
        if length:
            mappings.append(
                OutputMapping(
                    output_name="output",
                    selection=SourceSelection(start_byte=new_offset, end_byte=new_offset + length),
                    relation="identity",
                    origins=(_selection(base_ref, old_offset, change.start_byte),),
                )
            )
            new_offset += length
        size = len(change.replacement_bytes)
        if size:
            mappings.append(
                OutputMapping(
                    output_name="output",
                    selection=SourceSelection(start_byte=new_offset, end_byte=new_offset + size),
                    relation="derived",
                    origins=(ContentSelection(artifact=model_ref),),
                )
            )
            new_offset += size
        old_offset = change.end_byte
    length = len(base) - old_offset
    if length:
        mappings.append(
            OutputMapping(
                output_name="output",
                selection=SourceSelection(start_byte=new_offset, end_byte=new_offset + length),
                relation="identity",
                origins=(_selection(base_ref, old_offset, len(base)),),
            )
        )
    return tuple(mappings)


class WritingOperations:
    """Produce, read and revise completed writing drafts without preserving a task to resume."""

    def __init__(
        self,
        core: WrittenRecords,
        reader: RecordReader,
        generation: GenerationOperations,
        drafts: DraftStore,
        records: RecordOperations,
        materials: Materials,
    ) -> None:
        self.core, self.reader, self.generation = core, reader, generation
        self.drafts, self.records = drafts, records
        self.materials = materials
        self.execution, self.database, self.results = (
            reader.execution,
            reader.database,
            generation.results,
        )

    async def _choices(
        self, value: tuple[SourceChoice, ...] | ExternalInput, call: CallContext
    ) -> tuple[SourceChoice, ...]:
        if isinstance(value, ExternalInput):
            selected = await call.inputs.read_text(
                value.key, value.selection, call.settings.values.content.text_read_max_bytes
            )
            return TypeAdapter(tuple[SourceChoice, ...]).validate_json(selected.data)
        return value

    async def _base(self, ref: RecordRevisionRef, namespace: NamespaceRef) -> RecordView:
        await self.core.scope.resolve_for(
            namespace,
            NamespaceRef(instance_id=ref.record_ref.instance_id, id=ref.record_ref.namespace_id),
        )
        return await self.reader.read(ref, namespace=namespace.id)

    async def _feedback(
        self, base: RecordRevisionRef, refs: Sequence[str], mappings: Mapping[str, str]
    ) -> str:
        views = await self.records.feedback.select_for_edit(refs, base, mappings=mappings)
        return "\n\n".join(
            f"[{item.identifier}] {item.category}: {item.value or ''}\n{item.comment or ''}"
            for item in views
        )

    async def _prepare(
        self, request: WriteDraft, call: CallContext
    ) -> tuple[WritingProductContext, SelectedInputs, str, str, str, str]:
        target = (
            request.base if isinstance(request, (EditDraftRequest, RewriteDraftRequest)) else None
        )
        if isinstance(request, ReflectDraftRequest):
            if not request.journals or not request.topic.strip():
                raise NousError(
                    "input.reflection_required", "Reflection requires Journals and a topic"
                )
            target = request.journals[0]
        owner = (
            NamespaceRef(
                instance_id=target.record_ref.instance_id, id=target.record_ref.namespace_id
            )
            if target
            else None
        )
        namespace = await self.core.scope.resolve_for(request.namespace, owner)
        choices = await self._choices(request.sources, call)
        inherited: tuple[SourceBinding, ...] = ()
        inherited_materials: tuple[MaterialSource, ...] = ()
        targets: dict[str, ContentSelection] = {}
        selected_targets: tuple[ContentSelection, ...] = ()
        feedback_mappings: dict[str, str] = {}
        inputs: list[SelectedSource] = []
        base: RecordView | None = None
        feedback = ""
        if isinstance(request, (EditDraftRequest, RewriteDraftRequest)):
            base = await self._base(request.base, namespace)
            inherited = base.sources
            async with self.database.read() as tx:
                inherited_materials = (
                    await self.core.reader.material_sources_many_in(
                        tx,
                        (base.revision_ref.revision_id,),
                        sources={base.revision_ref.revision_id: base.sources},
                    )
                )[base.revision_ref.revision_id]
            if isinstance(request, RewriteDraftRequest) and base.ref.kind != "journal":
                raise NousError("input.invalid_kind", "Rewrite requires a Journal")
            if not request.instruction.strip() and not (
                isinstance(request, EditDraftRequest) and request.feedback
            ):
                raise NousError(
                    "input.instruction_required", "Provide a writing instruction or feedback"
                )
        if isinstance(request, NewDraftRequest):
            prepared = await self.core.sources.prepare(
                namespace, choices, reject_journals=True, input_reader=call.inputs
            )
        elif isinstance(request, ReflectDraftRequest):
            for revision in request.journals:
                original = await self._base(revision, namespace)
                if original.ref.kind != "journal":
                    raise NousError("input.invalid_kind", "Reflection requires Journals")
            first = await self.core.sources.prepare(namespace, request.journals, prefix="J")
            extra = await self.core.sources.prepare(
                namespace, choices, prefix="N", input_reader=call.inputs
            )
            prepared = PreparedSources(
                first.namespace,
                (*first.bindings, *extra.bindings),
                SelectedInputs(
                    sources=(*first.selected_inputs.sources, *extra.selected_inputs.sources),
                    facts=(*first.selected_inputs.facts, *extra.selected_inputs.facts),
                ),
                (*first.material_sources, *extra.material_sources),
            )
        else:
            prepared = await self.core.sources.prepare(
                namespace,
                choices,
                prefix="N",
                reserved_keys=[item.key for item in inherited],
                input_reader=call.inputs,
            )
        if isinstance(request, EditDraftRequest):
            assert base is not None
            if isinstance(request.targets, ExternalInput):
                selected = await call.inputs.read_text(
                    request.targets.key,
                    request.targets.selection,
                    call.settings.values.content.text_read_max_bytes,
                )
                selected_targets = TypeAdapter(tuple[ContentSelection, ...]).validate_json(
                    selected.data
                )
            else:
                selected_targets = request.targets
            if not selected_targets:
                raise NousError("content.edit_scope_invalid", "Provide at least one editable range")
            view = await self.results.artifacts.text_view(base.body_artifact)
            for index, selected in enumerate(selected_targets, 1):
                view.selection_bounds(selected)
                key = f"T{index}"
                targets[key] = selected
                inputs.append(
                    SelectedSource(
                        slot_key=key,
                        role="edit_target",
                        selections=(selected,),
                        display_name=base.title,
                    )
                )
            if isinstance(request.feedback_mappings, ExternalInput):
                selected = await call.inputs.read_text(
                    request.feedback_mappings.key,
                    request.feedback_mappings.selection,
                    call.settings.values.content.text_read_max_bytes,
                )
                feedback_mappings = TypeAdapter(dict[str, str]).validate_json(selected.data)
            else:
                feedback_mappings = request.feedback_mappings
            feedback = await self._feedback(request.base, request.feedback, feedback_mappings)
        elif isinstance(request, RewriteDraftRequest):
            assert base is not None
            inputs.append(
                SelectedSource(
                    slot_key="BASE",
                    role="revision_base",
                    selections=(base.body_selection,),
                    display_name=base.title,
                )
            )
        inputs.extend(prepared.selected_inputs.sources)
        if not inputs:
            raise NousError("input.sources_required", "Provide material for the writing request")
        settings = call.settings.values.cognition
        if isinstance(request, NewDraftRequest):
            task, contract, question = (
                request.task or settings.journalize.task,
                "journal-draft",
                request.topic or "",
            )
            instructions = f"Purpose: {request.purpose or ''}\nAuthor: {request.author or ''}\nViewpoint: {request.viewpoint or ''}"
        elif isinstance(request, EditDraftRequest):
            task, contract, question = (
                request.task or settings.edit.task,
                "journal-edit",
                request.instruction,
            )
            instructions = (
                f"Editable target slots: {', '.join(targets)}.\n"
                f"Existing citation directory: {_source_directory(inherited)}\n"
                "Preserve existing source links; new source keys are the N groups shown below.\n"
                f"Feedback opinions (not external facts):\n{feedback}"
            )
        elif isinstance(request, RewriteDraftRequest):
            task, contract, question = (
                request.task or settings.rewrite.task,
                "journal-rewrite",
                request.instruction,
            )
            instructions = (
                f"Existing citation directory: {_source_directory(inherited)}\n"
                "BASE is the complete exact draft. Preserve its source keys; new N keys are supplied separately."
            )
        else:
            task, contract, question = (
                request.task or settings.reflection.task,
                "journal-reflect",
                request.topic,
            )
            instructions = (
                f"Author: {request.author or ''}\nViewpoint: {request.viewpoint or ''}\n"
                "Write a new Journal and cite only the actual J/N source groups supplied here."
            )
        self.generation.require_contract(task, contract, call.settings)
        if isinstance(request, (NewDraftRequest, ReflectDraftRequest)):
            purpose, author, viewpoint = request.purpose, request.author, request.viewpoint
            times = await self.core.prepare_times(request.times)
        else:
            assert base is not None
            purpose, author, viewpoint, times = (
                base.purpose,
                base.author,
                base.viewpoint,
                base.times,
            )
        context = WritingProductContext(
            namespace_ref=namespace,
            intent=request.intent,
            base_revision=base.revision_ref if base else None,
            selected_sources=prepared.bindings,
            material_sources=(*inherited_materials, *prepared.material_sources),
            citation_context=CitationContext(
                inherited_sources=inherited, new_sources=prepared.bindings, targets=targets
            ),
            selected_edit_ranges=selected_targets,
            feedback_refs=tuple(dict.fromkeys(request.feedback))
            if isinstance(request, EditDraftRequest)
            else (),
            feedback_mappings=feedback_mappings,
            instruction=request.instruction
            if isinstance(request, (EditDraftRequest, RewriteDraftRequest))
            else None,
            change_kind=request.change_kind
            if isinstance(request, (EditDraftRequest, RewriteDraftRequest))
            else "edit",
            topic=request.topic
            if isinstance(request, (NewDraftRequest, ReflectDraftRequest))
            else None,
            purpose=purpose,
            author=author,
            viewpoint=viewpoint,
            times=times,
            original_title=base.title if base else None,
            title=request.title
            if isinstance(request, (NewDraftRequest, ReflectDraftRequest))
            else None,
            output_state="material",
        )
        return (
            context,
            SelectedInputs(sources=tuple(inputs), facts=prepared.selected_inputs.facts),
            task,
            contract,
            question,
            instructions,
        )

    async def generate(self, request: WriteDraft, call: CallContext) -> WritingOutcome:
        """Save the model product before attempting local composition or adoption."""
        async with self.execution.operation("writing.generate"):
            if request.adoption == "save":
                self.records.maintenance.validate_after(request.after)
            elif request.after.refresh == "run":
                raise NousError("input.refresh_requires_save", "Refresh requires save adoption")
            context, inputs, task, _, question, instructions = await self._prepare(request, call)
            completed = await self.generation.compute(
                GenerationRequest(
                    task=task,
                    inputs=inputs,
                    question=question,
                    instructions=instructions,
                    stream=request.stream,
                ),
                call,
            )
            output = completed.outputs.get("output")
            if context.intent == "edit":
                if output is None:
                    raise NousError("model.invalid_output", "Edit task returned no proposal")
                EditProposal.model_validate_json(output.data)
                output_state = "material"
            else:
                output_state = (
                    "material"
                    if writing_body(context.intent, output) is not None
                    else "no_material"
                )
            context = context.model_copy(update={"output_state": output_state})
            facts = completed.facts.model_copy(
                update={
                    "parameters": {
                        **completed.facts.parameters,
                        "writing": context.model_dump(mode="json"),
                    }
                }
            )
            product = await self.results.adopt(facts=facts, outputs=completed.outputs)
            await call.saved(product.ref)
            try:
                outcome = await self.draft_from_result(DraftFromResult(result=product.ref), call)
                if outcome.draft is None or request.adoption == "draft":
                    return outcome
                adopted = await self.records.adopt_draft(
                    AdoptDraft(draft=outcome.draft, after=request.after), call
                )
                return WritingOutcome(
                    status="adopted" if adopted.commit.changed else "unchanged",
                    product=product.ref,
                    draft=outcome.draft,
                    record=adopted,
                )
            except BaseException as error:
                add_error_context(error, {"product": product.ref.model_dump(mode="json")})
                raise

    async def _bindings(
        self, product: ProcessingResult, context: WritingProductContext
    ) -> tuple[SourceBinding, ...]:
        if not context.selected_sources:
            actual: tuple[SourceBinding, ...] = ()
        else:
            ns = await self.core.scope.read(context.namespace_ref)
            prepared = PreparedSources(ns, context.selected_sources, product.selected_inputs)
            actual = await self.core.sources.actual_bindings(product.inputs, prepared)
        return (*context.citation_context.inherited_sources, *actual)

    async def _edit_targets(
        self, product: ProcessingResult, context: WritingProductContext, base: RecordView
    ) -> dict[str, tuple[ContentSelection, ...]]:
        targets: dict[str, list[ContentSelection]] = {}
        for group in product.inputs:
            if group.role != "edit_target":
                continue
            allowed = context.citation_context.targets.get(group.slot_key)
            if allowed is None:
                raise NousError("cognition.source_mismatch", "Edit input has no selected target")
            for part in group.parts:
                origins = tuple(
                    item.origin
                    for item in part.origins
                    if item.origin.artifact == base.body_artifact
                )
                if not origins and part.content.artifact == base.body_artifact:
                    origins = (part.content,)
                if not origins:
                    origins = tuple(
                        item.origin
                        for item in await self.results.artifacts.map_origins(
                            part.content, base.body_artifact
                        )
                    )
                for origin in origins:
                    origin = origin.model_copy(
                        update={"selection": allowed.selection.restrict(origin.selection)}
                    )
                    for key in (group.key, group.slot_key):
                        if origin not in targets.setdefault(key, []):
                            targets[key].append(origin)
        return {key: tuple(values) for key, values in targets.items()}

    @staticmethod
    def _issues(text: str, sources: Sequence[SourceBinding]) -> tuple[OperationIssue, ...]:
        if not text.strip():
            raise NousError("input.empty_body", "A draft must contain Markdown")
        known = {source.key for source in sources}
        unknown = sorted({item.key for item in parse_citations(text).citations} - known)
        if unknown:
            return (
                OperationIssue(
                    stage="writing.compose",
                    code="cognition.source_unknown",
                    message="Draft references sources that are not bound",
                    details={"keys": list[JsonValue](unknown)},
                ),
            )
        return ()

    async def _save_composition(
        self,
        *,
        product: ResultRef,
        body: bytes,
        context: DraftContext,
        inputs: tuple[tuple[str, SourceRole, ContentSelection], ...],
        mappings: tuple[OutputMapping, ...],
        call: CallContext,
        parent: DraftRef | None = None,
        producer: Literal["program", "user"] = "program",
    ) -> DraftRef:
        selected = SelectedInputs(
            sources=tuple(
                SelectedSource(slot_key=key, role=role, selections=(selection,), display_name=key)
                for key, role, selection in inputs
            )
        )
        groups = tuple(
            InputGroup(
                key=key, slot_key=key, role=role, name=key, parts=(InputPart(content=selection),)
            )
            for key, role, selection in inputs
        )
        facts = ResultFacts(
            operation="writing.compose",
            producer=(
                ProgramProducer(operation="writing.compose")
                if producer == "program"
                else UserProducer()
            ),
            selected_inputs=selected,
            inputs=groups,
            mappings=mappings,
            dependencies=(product,),
            formed_at=self.results.artifacts.clock.utc_now(),
        )
        outputs = await self.results.prepare_outputs(
            {
                "output": OutputContent(
                    data=body, media_type="text/markdown", display_name="draft.md"
                )
            }
        )
        async with self.database.write() as tx:
            composition = await self.results.adopt_in(tx, facts=facts, prepared_outputs=outputs)
            ref = await self.drafts.create_in(
                tx,
                product=product,
                body=composition.outputs[0].artifact,
                context=context,
                composition=composition.ref,
                parent=parent,
            )
        await call.saved(composition.ref, ref)
        return ref

    async def draft_from_result(
        self, request: DraftFromResult, call: CallContext
    ) -> WritingOutcome:
        product = await self.results.get(request.result)
        raw_context = product.parameters.get("writing")
        if isinstance(raw_context, dict):
            context = WritingProductContext.model_validate(raw_context)
            await self.core.scope.resolve_for(request.namespace, context.namespace_ref)
        else:
            namespace = await self.core.scope.resolve_for(request.namespace)
            bindings = (
                request.sources
                if request.sources is not None
                else tuple(
                    SourceBinding(key=item.key, selections=item.sources)
                    for item in product.selections
                )
            )
            context = WritingProductContext(
                namespace_ref=namespace,
                intent="new",
                selected_sources=(),
                citation_context=CitationContext(inherited_sources=bindings),
                times=await self.core.prepare_times(None),
                title=request.title,
                output_state="material",
            )
        if request.sources is not None and isinstance(raw_context, dict):
            context = context.model_copy(
                update={
                    "selected_sources": (),
                    "citation_context": CitationContext(inherited_sources=request.sources),
                }
            )
        if request.title is not None:
            context = context.model_copy(update={"title": request.title})
        if context.output_state == "no_material":
            return WritingOutcome(status="no_material", product=product.ref)
        output = next(
            (item for item in product.outputs if item.name == (request.output or "output")), None
        )
        if output is None and request.output is None:
            markdown: list[ResultOutput] = []
            for item in product.outputs:
                artifact = await self.results.artifacts.get(item.artifact)
                if item.role != "diagnostic" and artifact.media_type == "text/markdown":
                    markdown.append(item)
            if len(markdown) == 1:
                output = markdown[0]
        if output is None:
            raise NousError(
                "cognition.markdown_output_required", "Choose one Markdown output for the draft"
            )
        if context.intent != "edit":
            artifact = await self.results.artifacts.get(output.artifact)
            if artifact.media_type != "text/markdown":
                raise NousError(
                    "cognition.markdown_output_required", "The selected output is not Markdown"
                )
        formal_sources = await self.materials.sources_for(
            ResultOutputRef(result=product.ref, output=output.name)
        )
        owner: NamespaceRef | None = None
        for source in formal_sources:
            reference = source.material
            if isinstance(reference, RecordRevisionRef):
                ref = reference.record_ref
            elif isinstance(reference, WorkRevisionRef):
                ref = reference.work_ref
            else:
                continue
            candidate = NamespaceRef(instance_id=ref.instance_id, id=ref.namespace_id)
            if owner is not None and owner != candidate:
                raise NousError(
                    "cognition.namespace_mismatch", "Material sources span different namespaces"
                )
            owner = candidate
        if formal_sources:
            namespace = await self.core.scope.resolve_for(request.namespace, owner)
            if isinstance(raw_context, dict):
                await self.core.scope.resolve_for(namespace, context.namespace_ref)
            else:
                context = context.model_copy(update={"namespace_ref": namespace})
            await self.materials.read_many(
                tuple(
                    MaterialSelection(material=item.material, selections=item.selections)
                    for item in formal_sources
                ),
                namespace=context.namespace_ref,
            )
            if not isinstance(raw_context, dict) and request.sources is None:
                sources = tuple(
                    SourceBinding(
                        key=item.key,
                        selections=item.selections,
                        owner_revision=item.material
                        if isinstance(item.material, RecordRevisionRef)
                        else None,
                        name=item.name,
                    )
                    for item in formal_sources
                )
                context = context.model_copy(
                    update={"citation_context": CitationContext(inherited_sources=sources)}
                )
        raw = await self.results.artifacts.read_bytes(output.artifact)
        bindings = await self._bindings(product, context)
        declared = {item.key: item for item in context.material_sources}
        declared.update({item.key: item for item in formal_sources})
        slots = {group.key: group.slot_key for group in product.inputs}
        adopted_sources: list[MaterialSource] = []
        for binding in bindings:
            item = declared.get(binding.key) or declared.get(slots.get(binding.key, ""))
            if item is not None and all(
                not subtract_selection(selection, item.selections)
                for selection in binding.selections
            ):
                adopted_sources.append(
                    item.model_copy(
                        update={
                            "key": binding.key,
                            "selections": binding.selections,
                            "kind": binding.usage,
                            "name": binding.name,
                        }
                    )
                )
        formal_sources = tuple(adopted_sources)
        keys = tuple(item.key for item in bindings)
        base = (
            await self._base(context.base_revision, context.namespace_ref)
            if context.base_revision
            else None
        )
        normalized_edits: tuple[NormalizedEdit, ...] = ()
        composition_inputs: tuple[tuple[str, SourceRole, ContentSelection], ...]
        if context.intent == "edit":
            assert base is not None
            proposal = EditProposal.model_validate_json(raw)
            proposal = proposal.model_copy(
                update={
                    "edits": [
                        item.model_copy(
                            update={
                                "after": normalize_source_links(
                                    item.after, keys, preserve=item.before
                                ).text
                            }
                        )
                        for item in proposal.edits
                    ]
                }
            )
            view = await self.results.artifacts.text_view(base.body_artifact)
            edited = compose_edit(proposal, view, await self._edit_targets(product, context, base))
            body = edited.utf8_bytes
            normalized_edits = tuple(
                NormalizedEdit(
                    start_byte=item.start_byte,
                    end_byte=item.end_byte,
                    before=item.expected_bytes.decode("utf-8"),
                    after=item.replacement_bytes.decode("utf-8"),
                )
                for item in edited.changes
            )
            mappings = _edit_mappings(
                base.body.encode("utf-8"), edited, base.body_artifact, output.artifact
            )
            composition_inputs = (
                ("BASE", "revision_base", ContentSelection(artifact=base.body_artifact)),
                ("PRODUCT", "context", ContentSelection(artifact=output.artifact)),
            )
        else:
            normalized = normalize_source_links(
                raw.decode("utf-8"), keys, preserve=base.body if base else None
            )
            body = normalized.text.encode("utf-8")
            mappings = _sequence_mappings(raw, body, output.artifact, changed_is_derived=True)
            composition_inputs = (
                ("PRODUCT", "context", ContentSelection(artifact=output.artifact)),
            )
        draft_context = DraftContext.model_validate(
            {
                **context.model_dump(mode="python"),
                "source_bindings": bindings,
                "material_sources": formal_sources,
                "normalized_edits": normalized_edits,
                "issues": self._issues(body.decode("utf-8"), bindings),
            }
        )
        if context.intent != "edit" and body == raw:
            async with self.database.write() as tx:
                ref = await self.drafts.create_in(
                    tx, product=product.ref, body=output.artifact, context=draft_context
                )
            await call.saved(ref)
        else:
            ref = await self._save_composition(
                product=product.ref,
                body=body,
                context=draft_context,
                inputs=composition_inputs,
                mappings=mappings,
                call=call,
            )
        return WritingOutcome(status="draft", product=product.ref, draft=ref)

    async def read_draft(self, request: ReadDraft, call: CallContext) -> DraftView:
        owner = (
            NamespaceRef(instance_id=request.draft.instance_id, id=request.draft.namespace_id)
            if isinstance(request.draft, DraftRef)
            else None
        )
        namespace = await self.core.scope.resolve_for(request.namespace, owner)
        return await self.drafts.read(
            request.draft, namespace=namespace.id, sources=request.sources
        )

    async def revise_draft(self, request: ReviseDraft, call: CallContext) -> DraftView:
        owner = (
            NamespaceRef(instance_id=request.draft.instance_id, id=request.draft.namespace_id)
            if isinstance(request.draft, DraftRef)
            else None
        )
        namespace = await self.core.scope.resolve_for(request.namespace, owner)
        old = await self.drafts.read(request.draft, namespace=namespace.id)
        body = (await call.inputs.body(request.markdown)).encode("utf-8")
        changes = request.sources
        if isinstance(changes, ExternalInput):
            selected = await call.inputs.read_text(
                changes.key, changes.selection, call.settings.values.content.text_read_max_bytes
            )
            changes = SourceChanges.model_validate_json(selected.data)
        bindings = (
            changes.apply_to(old.context.source_bindings)
            if changes
            else old.context.source_bindings
        )
        issues = list(self._issues(body.decode("utf-8"), bindings))
        unqualified = normalize_source_links(
            body.decode("utf-8"), tuple(item.key for item in bindings)
        )
        if unqualified.keys:
            issues.append(
                OperationIssue(
                    stage="writing.revise",
                    code="cognition.source_link_unqualified",
                    message="Source links require the source: prefix",
                    details={"keys": list(unqualified.keys)},
                )
            )
        context = old.context.model_copy(
            update={
                "source_bindings": bindings,
                "material_sources": tuple(
                    item
                    for item in old.context.material_sources
                    if (changes is None or changes.preserve and item.key not in changes.remove)
                    and any(
                        binding.key == item.key and binding.selections == item.selections
                        for binding in bindings
                    )
                ),
                "issues": tuple(issues),
            }
        )
        ref = await self._save_composition(
            product=old.result_ref,
            body=body,
            context=context,
            inputs=(("PARENT", "revision_base", ContentSelection(artifact=old.body_artifact)),),
            mappings=_sequence_mappings(
                old.body.encode("utf-8"), body, old.body_artifact, changed_is_derived=False
            ),
            parent=old.ref,
            producer="user",
            call=call,
        )
        return await self.drafts.read(ref, sources=True)


WRITE_DRAFT = define_operation(
    "writing.generate",
    request=WriteDraft,
    result=WritingOutcome,
    events=LanguageEvents,
    description="Generate a writing result and a draft from selected sources.",
)
DRAFT_FROM_RESULT = define_operation(
    "writing.draft_from_result",
    request=DraftFromResult,
    result=WritingOutcome,
    events=BasicEvents,
    description="Form a draft from an existing writing result.",
)
READ_DRAFT = define_operation(
    "writing.read_draft",
    request=ReadDraft,
    result=DraftView,
    events=BasicEvents,
    description="Read a draft, its sources and changes from its base.",
)
REVISE_DRAFT = define_operation(
    "drafts.revise",
    request=ReviseDraft,
    result=DraftView,
    events=BasicEvents,
    description="Save a revised draft while retaining its previous version.",
)
