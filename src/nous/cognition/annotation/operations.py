"""Persist annotation proposals, then atomically adopt exact Concept bindings and body positions."""

import hashlib
import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Literal
from uuid import uuid4

import sqlalchemy as sa
from pydantic import TypeAdapter
from sqlalchemy.dialects.sqlite import insert

from nous.application.contracts import BasicEvents, CallContext, LanguageEvents, define_operation
from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.operations import OperationIssue, add_error_context, record_completed
from nous.content.annotations import AnnotationProposal, AnnotationSet
from nous.content.contracts import (
    ContentSelection,
    ExternalInput,
    ResultFacts,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceSelection,
    UserProducer,
)
from nous.models.contracts import ReadingInput
from nous.models.generation import GenerationOperations, GenerationRequest
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx, WriteTx

from .. import concept_schema as schema
from ..concept_contracts import (
    AnnotationProposalView,
    AnnotationRef,
    AnnotationView,
    ConceptRef,
    ConceptView,
)
from ..concepts import ConceptOperations, ConceptTarget
from ..contracts import NamespaceRef, RecordRevisionRef, RevisionSelection
from ..writing.reading import RecordReader, Target
from .contracts import (
    AdoptAnnotation,
    AnnotateRecord,
    AnnotationCommit,
    AnnotationOutcome,
    AnnotationPlan,
    AnnotationProductContext,
    AnnotationProposed,
    AnnotationReady,
    AnnotationRequest,
    AnnotationSuperseded,
    ConceptCandidate,
    ConceptChoice,
    ProposeAnnotation,
    ReadAnnotation,
    ReadAnnotationProposal,
)
from .reading import read_views_in
from .task import ConceptAnnotationTask

_EXPECTED = object()


@dataclass(frozen=True)
class PreparedAnnotationAdoption:
    """Validated proposal and decisions for a short, caller-owned transaction."""

    target: Target
    proposal: AnnotationProposalView
    namespace: str | None
    bindings: Mapping[str, ConceptTarget | None]
    expected_active: str | None | object
    activate: bool
    activate_for_current: bool
    require_recall: bool
    parser_profile: str
    block_selections: Mapping[str, ContentSelection]


class AnnotationOperations:
    """Create durable proposals and adopt annotations for exact written revisions.

    Proposals retain the body revision, Concept candidates and active annotation
    observed during preparation. Adoption checks those facts before binding meanings.
    """

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        records: RecordReader,
        concepts: ConceptOperations,
        generation: GenerationOperations,
    ):
        self.database, self.execution, self.records = database, execution, records
        self.concepts, self.generation = concepts, generation
        self.results, self.artifacts = generation.results, generation.results.artifacts

    async def prepare_plan(
        self,
        target: RecordRevisionRef,
        request: AnnotationRequest,
        *,
        known_concepts: Mapping[str, ConceptTarget] | None = None,
    ) -> AnnotationPlan:
        """Capture one exact body, candidate meanings and active-pointer precondition."""
        async with self.execution.operation("annotation.prepare"):
            async with self.database.read() as tx:
                row, revision, ns = await self.records.resolve_in(
                    tx, target, target.record_ref.namespace_id
                )
                if (
                    request.eligibility != "historical"
                    and row["head_revision_id"] != revision["id"]
                ):
                    raise NousError(
                        "cognition.annotation_conflict", "Body changed before annotation"
                    )
                if request.eligibility == "current_recalled" and (
                    row["archived"] or not row["recall"]
                ):
                    raise NousError(
                        "cognition.annotation_conflict",
                        "Record is no longer eligible for automatic annotation",
                    )
                body = (await self.artifacts.get_in(tx, revision["body_artifact_id"])).ref
                active_id = await tx.connection.scalar(
                    sa.select(schema.annotation_heads.c.annotation_id).where(
                        schema.annotation_heads.c.record_revision_id == revision["id"]
                    )
                )
                active = (
                    (await self.read_in(tx, str(active_id), namespace=ns.id)).ref
                    if active_id
                    else None
                )
            if known_concepts is None:
                candidates = await self.concepts.candidates(
                    ns.id, await self.artifacts.read_text(body), request.candidate_limit
                )
                chosen = {f"C{index}": value for index, value in enumerate(candidates, 1)}
            else:
                async with self.database.read() as tx:
                    chosen = {
                        key: await self.concepts.read_in(tx, value, ns.id)
                        for key, value in known_concepts.items()
                    }
            if any(not key or any(char.isspace() for char in key) for key in chosen):
                raise NousError(
                    "input.invalid_candidate", "Concept candidate keys must be nonempty local keys"
                )
            return AnnotationPlan(
                target=target,
                body=RevisionSelection(
                    revision=target, selections=(ContentSelection(artifact=body),)
                ),
                task=request.task,
                candidates=tuple(
                    ConceptCandidate(key=key, concept=value) for key, value in chosen.items()
                ),
                expected_active=active,
                eligibility=request.eligibility,
            )

    async def produce_proposal(
        self, plan: AnnotationPlan, call: CallContext
    ) -> AnnotationProposalView:
        """Save a complete proposal with its exact body and observed candidates."""
        async with self.execution.operation("annotation.propose"):
            instructions = (
                "Existing exact Concept candidates; use reuse_key only for these keys:\n"
                + "\n".join(
                    f"{item.key}: {item.concept.label}\n{item.concept.description}"
                    f"\naliases: {', '.join(item.concept.aliases)}"
                    for item in plan.candidates
                )
            )
            selected = SelectedInputs(
                sources=(
                    SelectedSource(
                        slot_key="S1", role="revision_base", selections=plan.body.selections
                    ),
                )
            )
            completed = await self.generation.compute(
                GenerationRequest(task=plan.task, inputs=selected, instructions=instructions), call
            )
            annotation = AnnotationSet.model_validate_json(completed.outputs["output"].data)
            candidates = {item.key: item.concept.ref for item in plan.candidates}
            unknown = {item.reuse_key for item in annotation.concepts if item.reuse_key} - set(
                candidates
            )
            if unknown:
                raise NousError(
                    "cognition.unknown_candidate",
                    "Proposal uses unknown Concept candidate keys",
                    details={"keys": sorted(unknown)},
                )
            context = AnnotationProductContext(
                body_revision=plan.target,
                expected_active_annotation=plan.expected_active.id
                if plan.expected_active
                else None,
                candidates=candidates,
                eligibility=plan.eligibility,
            )
            facts = completed.facts.model_copy(
                update={
                    "parameters": {
                        **completed.facts.parameters,
                        "annotation_context": context.model_dump(mode="json"),
                    }
                }
            )
            result = await self.results.adopt(facts=facts, outputs=completed.outputs)
            await call.saved(result.ref)
            return await self.read_proposal(result.ref)

    async def _foreground_plan(
        self,
        target: Target,
        namespace: str | None,
        task: str | None,
        known_concepts: Mapping[str, ConceptTarget] | None,
        call: CallContext,
        *,
        eligibility: Literal["current_recalled", "current_any", "historical"] = "historical",
    ) -> AnnotationPlan:
        async with self.database.read() as tx:
            row, revision, _ = await self.records.resolve_in(tx, target, namespace)
            exact = self.records.revision_ref(row, revision)
        settings = call.settings.values.cognition.annotation
        task_id = task or settings.task
        self.generation.require_contract(task_id, "concept-annotation", call.settings)
        request = AnnotationRequest(
            task=task_id,
            candidate_limit=settings.known_concept_limit,
            eligibility=eligibility,
        )
        return await self.prepare_plan(exact, request, known_concepts=known_concepts)

    async def propose(
        self, request: ProposeAnnotation, call: CallContext
    ) -> AnnotationProposalView:
        """Generate or accept one proposal and save its complete interpretation context."""
        async with self.execution.operation("annotation.propose"):
            namespace = request.namespace
            if isinstance(namespace, NamespaceRef):
                if namespace.instance_id != self.artifacts.instance_id:
                    raise NousError(
                        "cognition.instance_mismatch", "Namespace belongs to another instance"
                    )
                namespace = namespace.id
            proposal = request.proposal
            if isinstance(proposal, ExternalInput):
                selected = await call.inputs.read_text(
                    proposal.key,
                    proposal.selection,
                    call.settings.values.content.text_read_max_bytes,
                )
                proposal = AnnotationProposal.model_validate_json(selected.data)
            if proposal is None:
                plan = await self._foreground_plan(
                    request.target, namespace, request.task, request.known_concepts, call
                )
                return await self.produce_proposal(plan, call)
            async with self.database.read() as tx:
                row, revision, ns = await self.records.resolve_in(tx, request.target, namespace)
                exact = self.records.revision_ref(row, revision)
                artifact = await self.artifacts.get_in(tx, revision["body_artifact_id"])
                expected = await tx.connection.scalar(
                    sa.select(schema.annotation_heads.c.annotation_id).where(
                        schema.annotation_heads.c.record_revision_id == revision["id"]
                    )
                )
                candidates = {
                    key: await self.concepts.read_in(tx, value, ns.id)
                    for key, value in (request.known_concepts or {}).items()
                }
            if any(not key or any(char.isspace() for char in key) for key in candidates):
                raise NousError(
                    "input.invalid_candidate", "Concept candidate keys must be nonempty local keys"
                )
            context = AnnotationProductContext(
                body_revision=exact,
                expected_active_annotation=expected,
                candidates={key: value.ref for key, value in candidates.items()},
                eligibility="historical",
            )
            definition = ConceptAnnotationTask()
            plans = await definition.prepare_sources(
                ((artifact, SourceSelection()),), self.artifacts
            )
            assert plans is not None
            reading = ReadingInput(sources=plans[0].batches[0].sources)
            validated = definition.validate(proposal.model_dump(mode="json"), reading)
            outputs = await definition.outputs(
                validated, reading, self.artifacts, structured=True, producer={"kind": "user"}
            )
            annotation = AnnotationSet.model_validate_json(outputs["output"].data)
            unknown = {
                concept.reuse_key for concept in annotation.concepts if concept.reuse_key
            } - set(candidates)
            if unknown:
                raise NousError(
                    "cognition.unknown_candidate",
                    "Proposal uses unknown Concept candidate keys",
                    details={"keys": sorted(unknown)},
                )
            inputs = tuple(
                group.model_copy(update={"role": "revision_base"})
                for group in plans[0].batches[0].inputs
            )
            slots = tuple(dict.fromkeys(group.slot_key for group in inputs))
            facts = ResultFacts(
                operation="annotation.manual",
                producer=UserProducer(),
                selected_inputs=SelectedInputs(
                    sources=tuple(
                        SelectedSource(
                            slot_key=slot,
                            role="revision_base",
                            selections=(ContentSelection(artifact=artifact.ref),),
                        )
                        for slot in slots
                    )
                ),
                inputs=inputs,
                parameters={"annotation_context": context.model_dump(mode="json")},
                coverage=plans[0].coverage,
                formed_at=self.artifacts.clock.utc_now(),
            )
            result = await self.results.adopt(facts=facts, outputs=outputs)
            await call.saved(result.ref)
            return await self.read_proposal(result.ref)

    async def read_proposal(self, reference: str | ResultRef) -> AnnotationProposalView:
        """Read a saved proposal and its frozen body and candidate context.

        Raises:
            NousError: The result lacks formal annotation context or its output.
        """
        async with self.execution.operation("annotation.read_proposal"):
            result = await self.results.get(reference)
            context = result.parameters.get("annotation_context")
            if not isinstance(context, dict):
                raise NousError(
                    "cognition.annotation_proposal_required",
                    "Result is not a formal annotation proposal",
                )
            output = next((output for output in result.outputs if output.name == "output"), None)
            if output is None:
                raise NousError(
                    "cognition.annotation_proposal_required", "Proposal output is missing"
                )
            annotation = AnnotationSet.model_validate_json(
                await self.artifacts.read_text(output.artifact)
            )
            product = AnnotationProductContext.model_validate(context)
            return AnnotationProposalView(
                result_ref=result.ref,
                body_revision=product.body_revision,
                expected_active_annotation=product.expected_active_annotation,
                candidates=product.candidates,
                annotation=annotation.model_copy(update={"result_ref": result.ref}),
            )

    async def prepare_adoption(
        self,
        target: Target,
        proposal_result: str | ResultRef,
        *,
        namespace: str | None = None,
        bindings: Mapping[str, ConceptTarget | None] | None = None,
        expected_active_annotation: str | None | object = _EXPECTED,
        activate: bool = True,
        activate_for_current: bool = True,
        require_recall: bool = False,
    ) -> PreparedAnnotationAdoption:
        """Validate the immutable proposal and prepare its exact source selections."""
        proposed = await self.read_proposal(proposal_result)
        body_view = await self.artifacts.text_view(proposed.annotation.body_ref)
        if body_view.parser_profile != proposed.annotation.parser_profile:
            raise NousError(
                "cognition.annotation_body", "Proposal parser profile differs from its body"
            )
        roots = [block for block in body_view.structure if block.parent_key is None]
        proposed.annotation.validate_references({block.key for block in roots})
        supplied = dict(bindings or {})
        unknown = set(supplied) - {concept.key for concept in proposed.annotation.concepts}
        if unknown:
            raise NousError(
                "cognition.annotation_binding",
                "Unknown local Concept bindings",
                details={"keys": sorted(unknown)},
            )
        return PreparedAnnotationAdoption(
            target=target,
            proposal=proposed,
            namespace=namespace,
            bindings=supplied,
            expected_active=expected_active_annotation,
            activate=activate,
            activate_for_current=activate_for_current,
            require_recall=require_recall,
            parser_profile=body_view.parser_profile,
            block_selections={
                block.key: ContentSelection(
                    artifact=body_view.body.artifact,
                    selection=SourceSelection(start_byte=block.start, end_byte=block.end),
                )
                for block in roots
            },
        )

    async def adopt_in(self, tx: WriteTx, prepared: PreparedAnnotationAdoption) -> AnnotationCommit:
        """Adopt or reuse the proposal, leaving commit and conflict policy to the caller."""
        proposed = prepared.proposal
        supplied = prepared.bindings
        block_selections = prepared.block_selections
        row, revision, ns = await self.records.resolve_in(tx, prepared.target, prepared.namespace)
        exact = self.records.revision_ref(row, revision)
        conflict: OperationIssue | None = None
        if exact.record_ref != proposed.body_revision.record_ref:
            raise NousError("cognition.annotation_body", "Proposal belongs to another record")
        if exact != proposed.body_revision:
            conflict = OperationIssue(
                stage="annotation",
                code="cognition.annotation_conflict",
                message="Body changed while annotation was being prepared",
                details={
                    "proposal_result": proposed.result_ref.identifier,
                    "target_revision": proposed.body_revision.identifier,
                    "current_revision": exact.identifier,
                },
            )
            row, revision, ns = await self.records.resolve_in(
                tx, proposed.body_revision, prepared.namespace
            )
            exact = self.records.revision_ref(row, revision)
        if revision["body_artifact_id"] != proposed.annotation.body_ref.id:
            raise NousError(
                "cognition.annotation_body", "Proposal belongs to a different exact body revision"
            )
        resolved: dict[str, ConceptView | None] = {}
        for concept in proposed.annotation.concepts:
            choice = supplied.get(concept.key, proposed.candidates.get(concept.reuse_key or ""))
            if concept.key not in supplied and concept.reuse_key and choice is None:
                raise NousError(
                    "cognition.unknown_candidate",
                    "Proposal candidate no longer resolves",
                )
            if isinstance(choice, str) and choice in proposed.candidates:
                choice = proposed.candidates[choice]
            if isinstance(choice, ConceptRef):
                raise NousError(
                    "cognition.exact_concept_required",
                    "Reuse requires an exact Concept revision",
                )
            if isinstance(choice, str) and "@" not in choice:
                raise NousError(
                    "cognition.exact_concept_required",
                    "Reuse requires an exact Concept revision",
                )
            resolved[concept.key] = (
                await self.concepts.read_in(tx, choice, ns.id) if choice is not None else None
            )
        decision = {
            key: value.ref.revision_id if value else None for key, value in resolved.items()
        }
        adoption_key = hashlib.sha256(
            json.dumps(
                {
                    "body": exact.revision_id,
                    "proposal": proposed.result_ref.id,
                    "bindings": decision,
                },
                sort_keys=True,
            ).encode()
        ).hexdigest()
        existing = (
            await tx.connection.execute(
                sa.select(schema.annotations.c.id, schema.annotations.c.identifier).where(
                    schema.annotations.c.adoption_key == adoption_key
                )
            )
        ).one_or_none()
        if existing is not None:
            object_id, identifier = str(existing.id), str(existing.identifier)
        else:
            object_id = str(uuid4())
            identifier = await insert_identifier(
                tx,
                schema.annotations,
                "ann",
                {
                    "id": object_id,
                    "record_revision_id": revision["id"],
                    "body_artifact_id": proposed.annotation.body_ref.id,
                    "parser_profile": prepared.parser_profile,
                    "proposal_result_id": proposed.result_ref.id,
                    "adoption_key": adoption_key,
                    "recorded_at": self.artifacts.clock.utc_now().isoformat(),
                },
            )
            for concept in proposed.annotation.concepts:
                meaning = resolved[concept.key]
                if meaning is None:
                    meaning = await self.concepts.create_in(
                        tx,
                        ns.id,
                        concept.label,
                        concept.description,
                        producer_result_id=proposed.result_ref.id,
                        body_revision_id=revision["id"],
                    )
                await tx.connection.execute(
                    schema.bindings.insert().values(
                        annotation_id=object_id,
                        local_key=concept.key,
                        concept_revision_id=meaning.ref.revision_id,
                    )
                )
            for ordinal, occurrence in enumerate(proposed.annotation.occurrences):
                await tx.connection.execute(
                    schema.occurrences.insert().values(
                        annotation_id=object_id,
                        key=occurrence.key,
                        concept_key=occurrence.concept,
                        ordinal=ordinal,
                    )
                )
                for position, key in enumerate(occurrence.blocks):
                    await tx.connection.execute(
                        schema.blocks.insert().values(
                            annotation_id=object_id,
                            occurrence_key=occurrence.key,
                            ordinal=position,
                            block_key=key,
                            selection=block_selections[key].model_dump(mode="json"),
                        )
                    )
            for ordinal, path in enumerate(proposed.annotation.paths):
                await tx.connection.execute(
                    schema.paths.insert().values(
                        annotation_id=object_id,
                        key=path.key,
                        label=path.label,
                        ordinal=ordinal,
                    )
                )
                if path.occurrences:
                    await tx.connection.execute(
                        schema.path_steps.insert(),
                        [
                            dict(
                                annotation_id=object_id,
                                path_key=path.key,
                                ordinal=position,
                                occurrence_key=key,
                            )
                            for position, key in enumerate(path.occurrences)
                        ],
                    )

        current_view_changed = False
        activation: Literal["active_current", "active_historical", "inactive"] = "inactive"
        if prepared.activate:
            if prepared.require_recall and (row["archived"] or not row["recall"]):
                conflict = OperationIssue(
                    stage="annotation",
                    code="cognition.annotation_conflict",
                    message="Record is no longer eligible for automatic annotation",
                    details={"proposal_result": proposed.result_ref.identifier},
                )
            if prepared.activate_for_current and row["head_revision_id"] != revision["id"]:
                conflict = OperationIssue(
                    stage="annotation",
                    code="cognition.annotation_conflict",
                    message="Body changed while annotation was being prepared",
                    details={
                        "proposal_result": proposed.result_ref.identifier,
                        "target_revision": exact.identifier,
                        "current_revision_id": row["head_revision_id"],
                    },
                )
            active = await tx.connection.scalar(
                sa.select(schema.annotation_heads.c.annotation_id).where(
                    schema.annotation_heads.c.record_revision_id == revision["id"]
                )
            )
            expected = (
                proposed.expected_active_annotation
                if prepared.expected_active is _EXPECTED
                else prepared.expected_active
            )
            if active != object_id:
                active_identifier = (
                    await tx.connection.scalar(
                        sa.select(schema.annotations.c.identifier).where(
                            schema.annotations.c.id == active
                        )
                    )
                    if active
                    else None
                )
                if active != expected and active_identifier != expected:
                    conflict = OperationIssue(
                        stage="annotation",
                        code="cognition.annotation_conflict",
                        message="Active annotation changed since proposal preparation",
                        details={
                            "proposal_result": proposed.result_ref.identifier,
                            "expected_annotation": str(expected) if expected else None,
                            "current_annotation": active_identifier,
                        },
                    )
            if conflict is None:
                if active != object_id:
                    await tx.connection.execute(
                        insert(schema.annotation_heads)
                        .values(record_revision_id=revision["id"], annotation_id=object_id)
                        .on_conflict_do_update(
                            index_elements=[schema.annotation_heads.c.record_revision_id],
                            set_={"annotation_id": object_id},
                        )
                    )
                    if (
                        row["head_revision_id"] == revision["id"]
                        and row["recall"]
                        and not row["archived"]
                    ):
                        ns = await self.records.scope.advance_in(tx, ns.id)
                        current_view_changed = True
                activation = (
                    "active_current"
                    if row["head_revision_id"] == revision["id"]
                    else "active_historical"
                )
        view = await self.read_in(tx, identifier, namespace=ns.id)
        if view.active:
            activation = (
                "active_current"
                if row["head_revision_id"] == revision["id"]
                else "active_historical"
            )
        return AnnotationCommit(
            annotation=view.ref,
            proposal=proposed.result_ref,
            activation=activation,
            current_view_changed=current_view_changed,
            namespace=ns.ref,
            generation=ns.content_generation,
            conflict=conflict,
        )

    async def _adopt(
        self,
        target: Target,
        proposal_result: str | ResultRef,
        *,
        namespace: str | None = None,
        bindings: Mapping[str, ConceptTarget | None] | None = None,
        expected_active_annotation: str | None | object = _EXPECTED,
        activate: bool = True,
        activate_for_current: bool = True,
        require_recall: bool = False,
    ) -> AnnotationView:
        """Strictly adopt a saved proposal; a conflict rolls back this adoption."""
        async with self.execution.operation("annotation.adopt"):
            prepared = await self.prepare_adoption(
                target,
                proposal_result,
                namespace=namespace,
                bindings=bindings,
                expected_active_annotation=expected_active_annotation,
                activate=activate,
                activate_for_current=activate_for_current,
                require_recall=require_recall,
            )
            try:
                async with self.database.write() as tx:
                    committed = await self.adopt_in(tx, prepared)
                    if committed.conflict is not None:
                        issue = committed.conflict
                        raise NousError(issue.code, issue.message, details=issue.details)
                record_completed(committed.annotation)
                return await self.read(committed.annotation, namespace=committed.namespace.id)
            except BaseException as error:
                add_error_context(
                    error,
                    {
                        "proposal_result": prepared.proposal.result_ref.identifier,
                        "body_revision": prepared.proposal.body_revision.identifier,
                    },
                )
                raise

    async def adopt(self, request: AdoptAnnotation, call: CallContext) -> AnnotationView:
        namespace = request.namespace
        if isinstance(namespace, NamespaceRef):
            if namespace.instance_id != self.artifacts.instance_id:
                raise NousError(
                    "cognition.instance_mismatch", "Namespace belongs to another instance"
                )
            namespace = namespace.id
        bindings = request.bindings
        if isinstance(bindings, ExternalInput):
            selected = await call.inputs.read_text(
                bindings.key, bindings.selection, call.settings.values.content.text_read_max_bytes
            )
            bindings = TypeAdapter(dict[str, ConceptChoice | None]).validate_json(selected.data)
        result = await self._adopt(
            request.target,
            request.proposal,
            namespace=namespace,
            bindings=bindings,
            activate=request.activate,
            activate_for_current=request.activate_for_current,
            require_recall=request.require_recall,
        )
        await call.saved(result.ref)
        return result

    async def read(
        self, target: str | AnnotationRef, *, namespace: str | None = None
    ) -> AnnotationView:
        """Read an adopted annotation by identifier or reference in the selected namespace."""
        async with self.execution.operation("annotation.read"):
            async with self.database.read() as tx:
                return await self.read_in(tx, target, namespace=namespace)

    async def read_annotation(self, request: ReadAnnotation, call: CallContext) -> AnnotationView:
        namespace = request.namespace
        if isinstance(namespace, NamespaceRef):
            if namespace.instance_id != self.artifacts.instance_id:
                raise NousError(
                    "cognition.instance_mismatch", "Namespace belongs to another instance"
                )
            namespace = namespace.id
        return await self.read(request.ref, namespace=namespace)

    async def read_annotation_proposal(
        self, request: ReadAnnotationProposal, call: CallContext
    ) -> AnnotationProposalView:
        return await self.read_proposal(request.ref)

    async def read_many_in(
        self, tx: ReadTx, revisions: Sequence[RecordRevisionRef]
    ) -> dict[str, AnnotationView]:
        """Capture active exact-body annotations, meanings and ordered selections in one read view.

        The returned mapping is keyed by record revision ID; revisions without an
        active annotation are absent. The caller owns the read transaction.
        """
        requested: dict[str, RecordRevisionRef] = {}
        for ref in revisions:
            if ref.record_ref.instance_id != self.artifacts.instance_id:
                raise NousError("cognition.instance_mismatch", "Record belongs to another instance")
            previous = requested.setdefault(ref.revision_id, ref)
            if previous != ref:
                raise NousError(
                    "cognition.revision_mismatch", "Conflicting exact revision references"
                )
        object_ids = list(requested)
        heads: dict[str, str] = {}
        for start in range(0, len(object_ids), 400):
            rows = (
                await tx.connection.execute(
                    sa.select(schema.annotation_heads).where(
                        schema.annotation_heads.c.record_revision_id.in_(
                            object_ids[start : start + 400]
                        )
                    )
                )
            ).mappings()
            heads.update((row["record_revision_id"], row["annotation_id"]) for row in rows)
        views = await read_views_in(
            tx, list(heads.values()), self.artifacts.instance_id, self.concepts
        )
        output: dict[str, AnnotationView] = {}
        for revision_id, annotation_id in heads.items():
            view = views[annotation_id]
            if view.ref.body_revision != requested[revision_id]:
                raise NousError(
                    "cognition.revision_mismatch", "Annotation belongs to another exact revision"
                )
            output[revision_id] = view
        return output

    async def read_in(
        self, tx: ReadTx, target: str | AnnotationRef, *, namespace: str | None = None
    ) -> AnnotationView:
        """Read an adopted annotation using the caller's read transaction."""
        if isinstance(target, AnnotationRef):
            if target.instance_id != self.artifacts.instance_id:
                raise NousError(
                    "cognition.instance_mismatch", "Annotation belongs to another instance"
                )
            namespace = namespace or target.namespace_id
            object_id = target.id
        else:
            object_id = target
        object_id = await tx.connection.scalar(
            sa.select(schema.annotations.c.id).where(
                sa.or_(
                    schema.annotations.c.id == object_id,
                    schema.annotations.c.identifier == object_id,
                )
            )
        )
        if object_id is None:
            raise NousError("cognition.annotation_not_found", "Annotation does not exist")
        view = (await read_views_in(tx, [object_id], self.artifacts.instance_id, self.concepts))[
            object_id
        ]
        await self.records.resolve_in(
            tx,
            view.ref.body_revision,
            namespace or self.execution.snapshot.values.cognition.default_namespace,
        )
        if isinstance(target, AnnotationRef) and view.ref != target:
            raise NousError(
                "cognition.annotation_mismatch", "Annotation reference differs from its identity"
            )
        return view

    async def annotate(self, request: AnnotateRecord, call: CallContext) -> AnnotationOutcome:
        """Save one proposal and optionally adopt it against the captured revision."""
        async with self.execution.operation("annotation.annotate"):
            if request.proposal is not None and request.proposal_result is not None:
                raise NousError(
                    "input.conflicting_proposal", "Choose a proposal or proposal result"
                )
            namespace = request.namespace
            if isinstance(namespace, NamespaceRef):
                if namespace.instance_id != self.artifacts.instance_id:
                    raise NousError(
                        "cognition.instance_mismatch", "Namespace belongs to another instance"
                    )
                namespace = namespace.id
            if request.proposal_result is not None:
                proposed = await self.read_proposal(request.proposal_result)
            elif request.proposal is None:
                eligibility = (
                    "current_recalled"
                    if request.require_recall
                    else "current_any"
                    if request.activate_for_current
                    else "historical"
                )
                plan = await self._foreground_plan(
                    request.target,
                    namespace,
                    request.task,
                    request.known_concepts,
                    call,
                    eligibility=eligibility,
                )
                proposed = await self.produce_proposal(plan, call)
            else:
                proposed = await self.propose(
                    ProposeAnnotation(
                        target=request.target,
                        namespace=namespace,
                        known_concepts=request.known_concepts,
                        proposal=request.proposal,
                        task=request.task,
                    ),
                    call,
                )
            if request.draft_only:
                return AnnotationProposed(proposal_ref=proposed.result_ref)
            try:
                adopted = await self.adopt(
                    AdoptAnnotation(
                        target=request.target
                        if request.proposal_result
                        else proposed.body_revision,
                        proposal=proposed.result_ref,
                        namespace=namespace,
                        bindings=request.bindings,
                        activate_for_current=request.activate_for_current,
                        require_recall=request.require_recall,
                    ),
                    call,
                )
            except BaseException as error:
                add_error_context(
                    error, {"proposal_result": proposed.result_ref.model_dump(mode="json")}
                )
                if isinstance(error, NousError) and error.code == "cognition.annotation_conflict":
                    return AnnotationSuperseded(proposal_ref=proposed.result_ref)
                raise
            return AnnotationReady(annotation_ref=adopted.ref, proposal_ref=proposed.result_ref)

    async def status(self) -> dict[str, int]:
        """Count adopted and active annotations across the instance."""
        async with self.execution.operation("annotation.status"):
            async with self.database.read() as tx:
                return {
                    "annotations": int(
                        await tx.connection.scalar(
                            sa.select(sa.func.count()).select_from(schema.annotations)
                        )
                        or 0
                    ),
                    "active_annotations": int(
                        await tx.connection.scalar(
                            sa.select(sa.func.count()).select_from(schema.annotation_heads)
                        )
                        or 0
                    ),
                }


PROPOSE_ANNOTATION = define_operation(
    "annotations.propose",
    request=ProposeAnnotation,
    result=AnnotationProposalView,
    events=LanguageEvents,
    description="Create an annotation proposal for a record revision.",
)
ADOPT_ANNOTATION = define_operation(
    "annotations.adopt",
    request=AdoptAnnotation,
    result=AnnotationView,
    events=BasicEvents,
    description="Adopt an annotation proposal.",
)
ANNOTATE_RECORD = define_operation(
    "annotations.annotate",
    request=AnnotateRecord,
    result=AnnotationOutcome,
    events=LanguageEvents,
    description="Annotate a record revision and adopt eligible results.",
)
READ_ANNOTATION = define_operation(
    "annotations.read",
    request=ReadAnnotation,
    result=AnnotationView,
    events=BasicEvents,
    description="Read an adopted annotation.",
)
READ_ANNOTATION_PROPOSAL = define_operation(
    "annotations.read_proposal",
    request=ReadAnnotationProposal,
    result=AnnotationProposalView,
    events=BasicEvents,
    description="Read an annotation proposal.",
)
