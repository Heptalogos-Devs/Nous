"""Persist annotation proposals, then atomically adopt exact Concept bindings and body positions."""

import hashlib
import json
from collections.abc import Mapping
from dataclasses import replace
from uuid import uuid4

import sqlalchemy as sa
from sqlalchemy.dialects.sqlite import insert

from nous.common.address import insert_address
from nous.common.errors import NousError
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
from .record_reading import Target, WrittenRecordReader

_EXPECTED = object()


class AnnotationOperations:
    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        records: WrittenRecordReader,
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
                                "proposal_result": proposed.result_ref.address,
                                "target_revision": proposed.body_revision.address,
                                "current_revision": exact.address,
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
                        sa.select(schema.annotations.c.address).where(
                            schema.annotations.c.adoption_key == adoption_key
                        )
                    )
                    if existing is not None:
                        address = str(existing)
                    else:
                        if activate_for_current and row["head_revision_id"] != revision["id"]:
                            raise NousError(
                                "cognition.annotation_conflict",
                                "Body changed while annotation was being prepared",
                                details={
                                    "proposal_result": proposed.result_ref.address,
                                    "target_revision": exact.address,
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
                            active_address = (
                                await tx.connection.scalar(
                                    sa.select(schema.annotations.c.address).where(
                                        schema.annotations.c.id == active
                                    )
                                )
                                if active
                                else None
                            )
                            if active_address != expected or expected is None:
                                raise NousError(
                                    "cognition.annotation_conflict",
                                    "Active annotation changed since proposal preparation",
                                    details={
                                        "proposal_result": proposed.result_ref.address,
                                        "expected_annotation": str(expected) if expected else None,
                                        "current_annotation": active_address,
                                    },
                                )
                        identifier = str(uuid4())
                        address = await insert_address(
                            tx,
                            schema.annotations,
                            "ann",
                            {
                                "id": identifier,
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
                                    annotation_id=identifier,
                                    local_key=concept.key,
                                    concept_revision_id=meaning.ref.revision_id,
                                )
                            )
                        for ordinal, occurrence in enumerate(proposed.annotation.occurrences):
                            await tx.connection.execute(
                                schema.occurrences.insert().values(
                                    annotation_id=identifier,
                                    key=occurrence.key,
                                    concept_key=occurrence.concept,
                                    ordinal=ordinal,
                                )
                            )
                            for position, key in enumerate(occurrence.blocks):
                                await tx.connection.execute(
                                    schema.blocks.insert().values(
                                        annotation_id=identifier,
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
                                    annotation_id=identifier,
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
                                            annotation_id=identifier,
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
                                .values(record_revision_id=revision["id"], annotation_id=identifier)
                                .on_conflict_do_update(
                                    index_elements=[schema.annotation_heads.c.record_revision_id],
                                    set_={"annotation_id": identifier},
                                )
                            )
                            if activate_for_current and row["recall"] and not row["archived"]:
                                await self.records.scope.advance_in(tx, ns.id)
                record_completed(address)
                return await self.read(
                    address, namespace=proposed.body_revision.record_ref.namespace_id
                )
            except BaseException as error:
                add_error_context(
                    error,
                    {
                        "proposal_result": proposed.result_ref.address,
                        "body_revision": proposed.body_revision.address,
                    },
                )
                raise

    async def read(
        self, target: str | AnnotationRef, *, namespace: str | None = None
    ) -> AnnotationView:
        async with self.execution.operation("annotation.read"):
            async with self.database.read() as tx:
                return await self.read_in(tx, target, namespace=namespace)

    async def read_in(
        self, tx: ReadTx, target: str | AnnotationRef, *, namespace: str | None = None
    ) -> AnnotationView:
        if isinstance(target, AnnotationRef):
            if target.instance_id != self.artifacts.instance_id:
                raise NousError(
                    "cognition.instance_mismatch", "Annotation belongs to another instance"
                )
            namespace = namespace or target.namespace_id
            target = target.id
        annotation = (
            (
                await tx.connection.execute(
                    sa.select(schema.annotations).where(
                        sa.or_(
                            schema.annotations.c.id == target,
                            schema.annotations.c.address == target,
                        )
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if annotation is None:
            raise NousError("cognition.annotation_not_found", "Annotation does not exist")
        # The immutable revision is looked up through its record, preserving namespace checks.
        from . import schema as written

        body = (
            (
                await tx.connection.execute(
                    sa.select(written.records.c.address, written.revisions.c.ordinal)
                    .join(written.revisions, written.revisions.c.record_id == written.records.c.id)
                    .where(written.revisions.c.id == annotation["record_revision_id"])
                )
            )
            .mappings()
            .one()
        )
        row, revision, ns = await self.records.resolve_in(
            tx, f"{body['address']}@{body['ordinal']}", namespace
        )
        raw_bindings = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.bindings,
                        schema.concepts.c.address,
                        schema.concept_revisions.c.ordinal,
                    )
                    .join(
                        schema.concept_revisions,
                        schema.concept_revisions.c.id == schema.bindings.c.concept_revision_id,
                    )
                    .join(
                        schema.concepts,
                        schema.concepts.c.id == schema.concept_revisions.c.concept_id,
                    )
                    .where(schema.bindings.c.annotation_id == annotation["id"])
                )
            )
            .mappings()
            .all()
        )
        meanings = {
            binding["local_key"]: await self.concepts.read_in(
                tx, f"{binding['address']}@{binding['ordinal']}", ns.id
            )
            for binding in raw_bindings
        }
        raw_occurrences = (
            (
                await tx.connection.execute(
                    sa.select(schema.occurrences)
                    .where(schema.occurrences.c.annotation_id == annotation["id"])
                    .order_by(schema.occurrences.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        raw_blocks = (
            (
                await tx.connection.execute(
                    sa.select(schema.blocks)
                    .where(schema.blocks.c.annotation_id == annotation["id"])
                    .order_by(schema.blocks.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        occurrences = tuple(
            BoundOccurrence(
                key=occurrence["key"],
                concept_key=occurrence["concept_key"],
                blocks=tuple(
                    block["block_key"]
                    for block in raw_blocks
                    if block["occurrence_key"] == occurrence["key"]
                ),
                selections=tuple(
                    ContentSelection.model_validate(block["selection"])
                    for block in raw_blocks
                    if block["occurrence_key"] == occurrence["key"]
                ),
            )
            for occurrence in raw_occurrences
        )
        raw_paths = (
            (
                await tx.connection.execute(
                    sa.select(schema.paths)
                    .where(schema.paths.c.annotation_id == annotation["id"])
                    .order_by(schema.paths.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        steps = (
            (
                await tx.connection.execute(
                    sa.select(schema.path_steps)
                    .where(schema.path_steps.c.annotation_id == annotation["id"])
                    .order_by(schema.path_steps.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        result = (
            (
                await tx.connection.execute(
                    sa.select(processing_results.c.id, processing_results.c.address).where(
                        processing_results.c.id == annotation["proposal_result_id"]
                    )
                )
            )
            .mappings()
            .one()
        )
        active = await tx.connection.scalar(
            sa.select(schema.annotation_heads.c.annotation_id).where(
                schema.annotation_heads.c.record_revision_id == revision["id"]
            )
        )
        return AnnotationView(
            ref=AnnotationRef(
                instance_id=self.artifacts.instance_id,
                namespace_id=ns.id,
                id=annotation["id"],
                address=annotation["address"],
                body_revision=self.records.revision_ref(row, revision),
            ),
            proposal_result=ResultRef(
                instance_id=self.artifacts.instance_id, id=result["id"], address=result["address"]
            ),
            parser_profile=annotation["parser_profile"],
            bindings=meanings,
            occurrences=occurrences,
            paths=tuple(
                BoundPath(
                    key=path["key"],
                    label=path["label"],
                    occurrences=tuple(
                        step["occurrence_key"] for step in steps if step["path_key"] == path["key"]
                    ),
                )
                for path in raw_paths
            ),
            active=active == annotation["id"],
            generation=ns.content_generation,
        )

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
    ) -> AnnotationProposalView | AnnotationView:
        async with self.execution.operation("annotation.annotate"):
            proposed = await self.propose(
                target,
                namespace=namespace,
                proposal=proposal,
                known_concepts=known_concepts,
                task=task,
            )
            if draft_only:
                return proposed
            return await self.adopt(
                proposed.body_revision, proposed.result_ref, namespace=namespace, bindings=bindings
            )

    async def status(self) -> dict[str, int]:
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
