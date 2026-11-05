"""Persist annotation proposals, then atomically adopt exact Concept bindings and body positions."""

import hashlib
import json
from collections import defaultdict
from collections.abc import Mapping, Sequence
from dataclasses import replace
from uuid import uuid4

import sqlalchemy as sa
from sqlalchemy.dialects.sqlite import insert
from sqlalchemy.engine import RowMapping

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.operations import add_error_context, record_completed
from nous.content.annotations import AnnotationProposal, AnnotationSet
from nous.content.contracts import (
    ContentSelection,
    InputSpec,
    ResultRef,
    SourceSelection,
)
from nous.content.schema import results as processing_results
from nous.models.contracts import ReadingInput
from nous.models.operations import GenerationOperations, ProducedTask
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx

from . import concept_schema as schema
from . import schema as written
from .concept_contracts import (
    AnnotationProposalView,
    AnnotationRef,
    AnnotationView,
    BoundOccurrence,
    BoundPath,
    ConceptRef,
    ConceptView,
)
from .concepts import ConceptOperations, ConceptTarget
from .contracts import RecordRef, RecordRevisionRef
from .writing.reading import RecordReader, Target

_EXPECTED = object()


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

    async def propose(
        self,
        target: Target,
        *,
        namespace: str | None = None,
        known_concepts: Mapping[str, ConceptTarget] | None = None,
        proposal: AnnotationProposal | None = None,
        task: str | None = None,
    ) -> AnnotationProposalView:
        """Prepare and save an annotation proposal for the resolved body revision.

        ``known_concepts`` maps local keys to existing Concepts or exact meanings; when
        both it and ``proposal`` are omitted, candidates are discovered from the body.
        A supplied proposal is validated locally; otherwise the configured task calls a
        model. The returned result reference can be used to adopt without another call.

        Raises:
            NousError: Candidate keys are invalid, the proposal refers to unknown
                candidates, or the body cannot be prepared as one complete batch.
        """
        async with self.execution.operation("annotation.propose"):
            async with self.database.read() as tx:
                row, revision, ns = await self.records.resolve_in(tx, target, namespace)
                exact = self.records.revision_ref(row, revision)
                artifact = await self.artifacts.get_in(tx, revision["body_artifact_id"])
                expected = await tx.connection.scalar(
                    sa.select(schema.annotation_heads.c.annotation_id).where(
                        schema.annotation_heads.c.record_revision_id == revision["id"]
                    )
                )
                candidates = {
                    key: await self.concepts.read_in(tx, value, ns.id)
                    for key, value in (known_concepts or {}).items()
                }
            settings = self.execution.snapshot.typed_values().cognition.annotation
            if known_concepts is None and proposal is None:
                discovered = await self.concepts.candidates(
                    ns.id,
                    await self.artifacts.read_text(artifact.ref),
                    settings.known_concept_limit,
                )
                candidates = {
                    f"C{index}": candidate for index, candidate in enumerate(discovered, 1)
                }
            if any(not key or any(char.isspace() for char in key) for key in candidates):
                raise NousError(
                    "input.invalid_candidate", "Concept candidate keys must be nonempty local keys"
                )
            context = {
                "body_revision": exact.model_dump(mode="json"),
                "expected_active_annotation": expected,
                "candidates": {
                    key: value.ref.model_dump(mode="json") for key, value in candidates.items()
                },
            }
            if proposal is None:
                prepared = await self.generation.prepare_input(
                    task or settings.task,
                    InputSpec(kind="artifact", artifact=artifact.ref, role="revision_base"),
                    self.execution.snapshot,
                )
                if len(prepared.batches) != 1:
                    raise NousError(
                        "cognition.annotation_body", "Annotation requires one complete body"
                    )
                instructions = "\n".join(
                    f"{key}: {candidate.label}\n{candidate.description}\n"
                    f"aliases: {', '.join(candidate.aliases)}"
                    for key, candidate in candidates.items()
                )
                produced = await self.generation.produce(
                    prepared.frozen,
                    prepared.batches[0],
                    self.execution.snapshot,
                    instructions="Existing exact Concept candidates; use reuse_key only for these keys:\n"
                    + instructions,
                )
                annotation = AnnotationSet.model_validate_json(produced.outputs["output"].data)
            else:
                definition = self.generation.executor.compiler.catalog.tasks["concept-annotation"]
                plans = await definition.prepare_sources(
                    ((artifact, SourceSelection()),), self.artifacts
                )
                assert plans is not None
                reading = ReadingInput(sources=plans[0].batches[0].sources)
                validated = definition.validate(proposal.model_dump(mode="json"), reading)
                outputs = await definition.outputs(
                    validated, reading, self.artifacts, structured=True, producer={"kind": "manual"}
                )
                annotation = AnnotationSet.model_validate_json(outputs["output"].data)
                produced = ProducedTask(
                    operation="annotation.manual",
                    producer={"kind": "manual"},
                    actual_inputs=tuple(
                        group.model_copy(update={"role": "revision_base"})
                        for group in plans[0].batches[0].inputs
                    ),
                    outputs=outputs,
                    request={},
                    parameters={},
                    coverage=plans[0].coverage,
                    completed_stage_refs=(),
                )
            unknown = {
                concept.reuse_key for concept in annotation.concepts if concept.reuse_key
            } - set(candidates)
            if unknown:
                raise NousError(
                    "cognition.unknown_candidate",
                    "Proposal uses unknown Concept candidate keys",
                    details={"keys": sorted(unknown)},
                )
            produced = replace(
                produced, parameters={**produced.parameters, "annotation_context": context}
            )
            result = await self.generation.adopt_produced(produced)
            return AnnotationProposalView(
                result_ref=result.ref,
                body_revision=exact,
                expected_active_annotation=expected,
                candidates={key: value.ref for key, value in candidates.items()},
                annotation=annotation.model_copy(update={"result_ref": result.ref}),
            )

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
            return AnnotationProposalView.model_validate(
                {
                    **context,
                    "result_ref": result.ref,
                    "annotation": annotation.model_copy(update={"result_ref": result.ref}),
                }
            )

    async def adopt(
        self,
        target: Target,
        proposal_result: str | ResultRef,
        *,
        namespace: str | None = None,
        bindings: Mapping[str, ConceptTarget | None] | None = None,
        expected_active_annotation: str | None | object = _EXPECTED,
        activate: bool = True,
        activate_for_current: bool = True,
    ) -> AnnotationView:
        """Adopt a saved proposal against its exact body in one write transaction.

        ``bindings`` maps local proposal keys to exact Concept revisions or ``None``
        for a new Concept. By default the expected active annotation is the one
        captured by the proposal. Repeating the same body, proposal and binding
        decisions reuses the existing annotation. A new adoption checks the active
        annotation and, when ``activate_for_current`` is true, the current body.
        On conflict the saved proposal remains available for inspection or retry.

        Returns:
            The adopted annotation and its exact body and Concept bindings.

        Raises:
            NousError: ``cognition.annotation_proposal_required`` if the result is not a
                saved proposal; ``cognition.annotation_body`` for a different body or
                parser profile; ``cognition.annotation_binding`` for unknown keys;
                ``cognition.exact_concept_required`` for a non-exact reuse choice; or
                ``cognition.annotation_conflict`` for a changed body or active annotation.
        """
        async with self.execution.operation("annotation.adopt"):
            proposed = await self.read_proposal(proposal_result)
            body_view = await self.artifacts.text_view(proposed.annotation.body_ref)
            if body_view.parser_profile != proposed.annotation.parser_profile:
                raise NousError(
                    "cognition.annotation_body", "Proposal parser profile differs from its body"
                )
            proposed.annotation.validate_references(
                {block.key for block in body_view.blocks if block.parent_key is None}
            )
            supplied = dict(bindings or {})
            unknown = set(supplied) - {concept.key for concept in proposed.annotation.concepts}
            if unknown:
                raise NousError(
                    "cognition.annotation_binding",
                    "Unknown local Concept bindings",
                    details={"keys": sorted(unknown)},
                )
            try:
                async with self.database.write() as tx:
                    row, revision, ns = await self.records.resolve_in(tx, target, namespace)
                    exact = self.records.revision_ref(row, revision)
                    if (
                        exact.record_ref == proposed.body_revision.record_ref
                        and exact != proposed.body_revision
                    ):
                        raise NousError(
                            "cognition.annotation_conflict",
                            "Body changed while annotation was being prepared",
                            details={
                                "proposal_result": proposed.result_ref.identifier,
                                "target_revision": proposed.body_revision.identifier,
                                "current_revision": exact.identifier,
                            },
                        )
                    if (
                        exact != proposed.body_revision
                        or revision["body_artifact_id"] != proposed.annotation.body_ref.id
                    ):
                        raise NousError(
                            "cognition.annotation_body",
                            "Proposal belongs to a different exact body revision",
                        )
                    resolved: dict[str, ConceptView | None] = {}
                    for concept in proposed.annotation.concepts:
                        choice = supplied.get(
                            concept.key, proposed.candidates.get(concept.reuse_key or "")
                        )
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
                            await self.concepts.read_in(tx, choice, ns.id)
                            if choice is not None
                            else None
                        )
                    decision = {
                        key: value.ref.revision_id if value else None
                        for key, value in resolved.items()
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
                    existing = await tx.connection.scalar(
                        sa.select(schema.annotations.c.identifier).where(
                            schema.annotations.c.adoption_key == adoption_key
                        )
                    )
                    if existing is not None:
                        identifier = str(existing)
                    else:
                        if activate_for_current and row["head_revision_id"] != revision["id"]:
                            raise NousError(
                                "cognition.annotation_conflict",
                                "Body changed while annotation was being prepared",
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
                            if expected_active_annotation is _EXPECTED
                            else expected_active_annotation
                        )
                        if active != expected:
                            active_identifier = (
                                await tx.connection.scalar(
                                    sa.select(schema.annotations.c.identifier).where(
                                        schema.annotations.c.id == active
                                    )
                                )
                                if active
                                else None
                            )
                            if active_identifier != expected or expected is None:
                                raise NousError(
                                    "cognition.annotation_conflict",
                                    "Active annotation changed since proposal preparation",
                                    details={
                                        "proposal_result": proposed.result_ref.identifier,
                                        "expected_annotation": str(expected) if expected else None,
                                        "current_annotation": active_identifier,
                                    },
                                )
                        object_id = str(uuid4())
                        identifier = await insert_identifier(
                            tx,
                            schema.annotations,
                            "ann",
                            {
                                "id": object_id,
                                "record_revision_id": revision["id"],
                                "body_artifact_id": proposed.annotation.body_ref.id,
                                "parser_profile": body_view.parser_profile,
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
                                        selection=body_view.block(key).selection.model_dump(
                                            mode="json"
                                        ),
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
                        if activate:
                            await tx.connection.execute(
                                insert(schema.annotation_heads)
                                .values(record_revision_id=revision["id"], annotation_id=object_id)
                                .on_conflict_do_update(
                                    index_elements=[schema.annotation_heads.c.record_revision_id],
                                    set_={"annotation_id": object_id},
                                )
                            )
                            if activate_for_current and row["recall"] and not row["archived"]:
                                await self.records.scope.advance_in(tx, ns.id)
                record_completed(identifier)
                return await self.read(
                    identifier, namespace=proposed.body_revision.record_ref.namespace_id
                )
            except BaseException as error:
                add_error_context(
                    error,
                    {
                        "proposal_result": proposed.result_ref.identifier,
                        "body_revision": proposed.body_revision.identifier,
                    },
                )
                raise

    async def read(
        self, target: str | AnnotationRef, *, namespace: str | None = None
    ) -> AnnotationView:
        """Read an adopted annotation by identifier or reference in the selected namespace."""
        async with self.execution.operation("annotation.read"):
            async with self.database.read() as tx:
                return await self.read_in(tx, target, namespace=namespace)

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
        views = await self._read_views_in(tx, list(heads.values()))
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
        view = (await self._read_views_in(tx, [object_id]))[object_id]
        await self.records.resolve_in(
            tx, view.ref.body_revision,
            namespace or self.execution.snapshot.typed_values().cognition.default_namespace,
        )
        if isinstance(target, AnnotationRef) and view.ref != target:
            raise NousError(
                "cognition.annotation_mismatch", "Annotation reference differs from its identity"
            )
        return view

    async def _read_views_in(
        self, tx: ReadTx, annotation_ids: Sequence[str]
    ) -> dict[str, AnnotationView]:
        """One materializer serves single reads and corpus capture; no content IO occurs here."""
        output: dict[str, AnnotationView] = {}
        object_ids = list(dict.fromkeys(annotation_ids))
        for start in range(0, len(object_ids), 400):
            chunk = object_ids[start : start + 400]
            annotations = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.annotations,
                            written.records.c.id.label("record_id"),
                            written.records.c.identifier.label("record_identifier"),
                            written.records.c.kind.label("record_kind"),
                            written.records.c.namespace_id,
                            written.revisions.c.ordinal.label("record_ordinal"),
                            written.namespaces.c.content_generation,
                            processing_results.c.identifier.label("proposal_identifier"),
                            schema.annotation_heads.c.annotation_id.label("active_annotation"),
                        )
                        .join(
                            written.revisions,
                            written.revisions.c.id == schema.annotations.c.record_revision_id,
                        )
                        .join(
                            written.records, written.records.c.id == written.revisions.c.record_id
                        )
                        .join(
                            written.namespaces,
                            written.namespaces.c.id == written.records.c.namespace_id,
                        )
                        .join(
                            processing_results,
                            processing_results.c.id == schema.annotations.c.proposal_result_id,
                        )
                        .outerjoin(
                            schema.annotation_heads,
                            schema.annotation_heads.c.record_revision_id == written.revisions.c.id,
                        )
                        .where(schema.annotations.c.id.in_(chunk))
                    )
                )
                .mappings()
                .all()
            )
            data: dict[str, list[RowMapping]] = {}
            for table in (
                schema.bindings,
                schema.occurrences,
                schema.blocks,
                schema.paths,
                schema.path_steps,
            ):
                query = sa.select(table).where(table.c.annotation_id.in_(chunk))
                if "ordinal" in table.c:
                    query = query.order_by(table.c.ordinal)
                data[table.name] = list((await tx.connection.execute(query)).mappings())
            meanings = await self.concepts.read_revisions_in(
                tx, [row["concept_revision_id"] for row in data[schema.bindings.name]]
            )
            bindings: dict[str, dict[str, ConceptView]] = defaultdict(dict)
            for row in data[schema.bindings.name]:
                bindings[row["annotation_id"]][row["local_key"]] = meanings[
                    row["concept_revision_id"]
                ]
            blocks: dict[tuple[str, str], list[RowMapping]] = defaultdict(list)
            for row in data[schema.blocks.name]:
                blocks[row["annotation_id"], row["occurrence_key"]].append(row)
            occurrences: dict[str, list[BoundOccurrence]] = defaultdict(list)
            for row in data[schema.occurrences.name]:
                parts = blocks[row["annotation_id"], row["key"]]
                selections = tuple(
                    ContentSelection.model_validate(part["selection"]) for part in parts
                )
                if not selections or any(
                    part.selection.start_byte is None or part.selection.end_byte is None
                    for part in selections
                ):
                    raise NousError(
                        "cognition.annotation_invalid", "Annotation has no exact block selection"
                    )
                occurrences[row["annotation_id"]].append(
                    BoundOccurrence(
                        key=row["key"],
                        concept_key=row["concept_key"],
                        blocks=tuple(part["block_key"] for part in parts),
                        selections=selections,
                    )
                )
            steps: dict[tuple[str, str], list[str]] = defaultdict(list)
            for row in data[schema.path_steps.name]:
                steps[row["annotation_id"], row["path_key"]].append(row["occurrence_key"])
            paths: dict[str, list[BoundPath]] = defaultdict(list)
            for row in data[schema.paths.name]:
                paths[row["annotation_id"]].append(
                    BoundPath(
                        key=row["key"],
                        label=row["label"],
                        occurrences=tuple(steps[row["annotation_id"], row["key"]]),
                    )
                )
            for row in annotations:
                record = RecordRef(
                    instance_id=self.artifacts.instance_id,
                    namespace_id=row["namespace_id"],
                    kind=row["record_kind"],
                    id=row["record_id"],
                    identifier=row["record_identifier"],
                )
                bound = bindings[row["id"]]
                if any(
                    meaning.ref.concept_ref.namespace_id != record.namespace_id
                    for meaning in bound.values()
                ):
                    raise NousError(
                        "cognition.namespace_mismatch",
                        "Annotation meaning belongs to another namespace",
                    )
                if any(
                    part.artifact.id != row["body_artifact_id"]
                    or part.artifact.instance_id != self.artifacts.instance_id
                    for occurrence in occurrences[row["id"]]
                    for part in occurrence.selections
                ):
                    raise NousError(
                        "cognition.annotation_body", "Annotation selection belongs to another body"
                    )
                output[row["id"]] = AnnotationView(
                    ref=AnnotationRef(
                        instance_id=self.artifacts.instance_id,
                        namespace_id=record.namespace_id,
                        id=row["id"],
                        identifier=row["identifier"],
                        body_revision=RecordRevisionRef(
                            record_ref=record,
                            revision_id=row["record_revision_id"],
                            ordinal=row["record_ordinal"],
                        ),
                    ),
                    proposal_result=ResultRef(
                        instance_id=self.artifacts.instance_id,
                        id=row["proposal_result_id"],
                        identifier=row["proposal_identifier"],
                    ),
                    parser_profile=row["parser_profile"],
                    bindings=bound,
                    occurrences=tuple(occurrences[row["id"]]),
                    paths=tuple(paths[row["id"]]),
                    active=row["active_annotation"] == row["id"],
                    generation=row["content_generation"],
                )
        missing = set(object_ids) - output.keys()
        if missing:
            raise NousError(
                "cognition.annotation_not_found",
                "Annotation does not exist",
                details={"annotation_ids": sorted(missing)},
            )
        return output

    async def annotate(
        self,
        target: Target,
        *,
        namespace: str | None = None,
        proposal: AnnotationProposal | None = None,
        known_concepts: Mapping[str, ConceptTarget] | None = None,
        bindings: Mapping[str, ConceptTarget | None] | None = None,
        draft_only: bool = False,
        task: str | None = None,
        proposal_result: str | None = None,
        activate_for_current: bool = True,
    ) -> AnnotationProposalView | AnnotationView:
        """Save a proposal and optionally adopt it for an exact record revision.

        Pass ``proposal_result`` to reuse a saved proposal without another model request.
        If adoption fails after proposal creation, the proposal remains saved.

        Returns:
            The saved proposal when ``draft_only`` is true; otherwise the adopted
            annotation with exact body and Concept bindings.

        Raises:
            NousError: ``input.conflicting_proposal`` if both proposal forms are given;
                ``cognition.annotation_proposal_required`` for a non-proposal result;
                ``cognition.annotation_conflict`` if adoption detects a changed body
                or active annotation; or proposal and binding validation errors.
        """
        async with self.execution.operation("annotation.annotate"):
            if proposal is not None and proposal_result is not None:
                raise NousError(
                    "input.conflicting_proposal", "Choose a proposal or proposal result"
                )
            proposed = (
                await self.read_proposal(proposal_result)
                if proposal_result
                else await self.propose(
                    target,
                    namespace=namespace,
                    proposal=proposal,
                    known_concepts=known_concepts,
                    task=task,
                )
            )
            if draft_only:
                return proposed
            return await self.adopt(
                target if proposal_result else proposed.body_revision,
                proposed.result_ref,
                namespace=namespace,
                bindings=bindings,
                activate_for_current=activate_for_current,
            )

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
