"""Creator documents use the shared typed proposal and exact revision adoption path."""

from nous_sdk.contracts import FreezeCatalog
from nous_sdk.contracts.ego_model import InterpretSeed, SaveSeed, SeedVersion
from nous_sdk.contracts.maintenance import MaintenanceRun, MaintenanceSubmission, StartMaintenance
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import JSON, Column, Integer, String, Table

from nous.cognition.ego_model import EgoService
from nous.cognition.records import CognitiveRecords
from nous.identity.access import require_namespace_in
from nous.imports.service import ImportService
from nous.maintenance.service import MaintenanceService
from nous.material.service import MaterialService
from nous.platform.call_context import CallContext
from nous.platform.content import ContentLease, ContentStore
from nous.platform.errors import DomainError
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

seed_versions = Table(
    "seed_versions",
    metadata,
    Column("namespace_id", String, primary_key=True),
    Column("seed_id", String, primary_key=True),
    Column("version", Integer, primary_key=True),
    Column("record", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)


class SeedService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        maintenance: MaintenanceService,
        imports: ImportService,
        *,
        ego: EgoService,
        materials: MaterialService,
        content: ContentStore,
    ) -> None:
        self.ego = ego
        self.materials, self.content = materials, content
        self.authority, self.cognition, self.maintenance = (authority, cognition, maintenance)
        self.imports = imports

    async def interpret(self, ctx: CallContext, command: InterpretSeed) -> MaintenanceSubmission:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx, "ego")
        existing = await self.maintenance.existing_seed(ctx, command.operation_id)
        if existing is not None:
            if (existing.command.instruction, existing.command.previous_seed_id) != (
                command.instruction,
                command.previous_seed_id,
            ):
                raise DomainError(
                    "seed_invocation_conflict",
                    "The existing seed interpretation has different input",
                )
            await self.imports.freeze_catalog(
                ctx,
                FreezeCatalog(
                    operation_id=command.operation_id + ":materials", inputs=command.documents
                ),
            )
            return MaintenanceSubmission(run_id=existing.id, job_id=existing.job_id)
        previous_references = ()
        if command.previous_seed_id is not None:
            previous = await self.maintenance.read(ctx, command.previous_seed_id)
            if (
                previous.command.kind != "seed"
                or previous.review is None
                or previous.review.decision != "adopt"
            ):
                raise DomainError(
                    "seed_version_invalid", "Previous seed must be an adopted version"
                )
            previous_references = previous.review.adopted
        frozen = FreezeCatalog(
            operation_id=command.operation_id + ":materials", inputs=command.documents
        )
        catalog = await self.imports.freeze_catalog(ctx, frozen)
        if not catalog.units or catalog.errors:
            raise DomainError(
                "seed_input_invalid", "Creator materials must have supported readable sources"
            )
        effective = ctx
        if ctx.source_groups():
            async with self.authority.read(ctx) as view:
                effective = (
                    await self.cognition.materials.restore_input_in(
                        view,
                        ctx,
                        observation_ids=tuple(
                            dict.fromkeys(
                                unit.observation_id for unit in catalog.units if unit.observation_id
                            )
                        ),
                    )
                ).context
        return await self.maintenance.submit(
            effective,
            StartMaintenance(
                operation_id=command.operation_id,
                instruction=command.instruction,
                references=previous_references,
                source_catalog_id=catalog.id,
                kind="seed",
                previous_seed_id=command.previous_seed_id,
                limit=1000,
            ),
        )

    async def import_and_adopt(self, ctx: CallContext, command: InterpretSeed) -> MaintenanceRun:
        submission = await self.interpret(ctx, command)
        current = await self.maintenance.read(ctx, submission.run_id)
        await self.maintenance.execution.wait(ctx, current.job_id)
        run = await self.maintenance.read(ctx, submission.run_id)
        if run.review is None:
            from nous_sdk.contracts.maintenance import ReviewMaintenance

            run = await self.maintenance.review(
                ctx,
                run.id,
                ReviewMaintenance(
                    operation_id=command.operation_id + ":adopt",
                    expected_revision=run.revision,
                    decision="adopt",
                    reason="Creator imported and adopted the interpreted seed",
                ),
            )
        return run

    async def save(self, ctx: CallContext, command: SaveSeed) -> SeedVersion:
        """Retain a complete creator-document version before any interpretation."""
        from uuid import uuid4

        from nous_sdk.contracts.media import MediaReference
        from sqlalchemy import func, select

        leases: list[ContentLease | None] = []
        try:
            for document in command.documents:
                leases.append(
                    await self.content.publish_bytes(
                        document.inline_text().encode("utf-8"), document.media_type
                    )
                    if not isinstance(document, MediaReference) and document.material_id is None
                    else None
                )
            async with self.authority.write(ctx) as tx:
                await require_namespace_in(tx, ctx, "ego")
                seed_id = command.seed_id or str(uuid4())
                version = (
                    await tx.connection.scalar(
                        select(func.max(seed_versions.c.version)).where(
                            seed_versions.c.namespace_id == ctx.namespace_id,
                            seed_versions.c.seed_id == seed_id,
                        )
                    )
                    or 0
                )
                if version != command.expected_version:
                    raise DomainError(
                        "seed_version_conflict", "Seed changed since the selected version"
                    )
                if version:
                    await self.read_in(tx, ctx, seed_id, version)
                sources: list[MediaReference] = []
                for document, lease in zip(command.documents, leases, strict=True):
                    if isinstance(document, MediaReference):
                        sources.append(
                            (await self.materials.provenance_in(tx, ctx, document)).reference
                        )
                        continue
                    if document.material_id is None:
                        admitted = await self.materials.admit_text_in(tx, ctx, document, lease)
                    else:
                        admitted = await self.materials.admit_observation_in(
                            tx, ctx, document.material_id, document
                        )
                    sources.append(
                        MediaReference(
                            kind="material",
                            id=admitted.material["id"],
                            observation_id=admitted.event["id"],
                        )
                    )
                result = SeedVersion(
                    seed_id=seed_id,
                    version=version + 1,
                    namespace_id=ctx.namespace_id,
                    instruction=command.instruction,
                    sources=tuple(sources),
                    created_by=ctx.caller_id,
                    created_at=tx.recorded_at,
                    commit_seq=tx.commit_seq,
                )
                await tx.connection.execute(
                    seed_versions.insert().values(
                        namespace_id=ctx.namespace_id,
                        seed_id=seed_id,
                        version=result.version,
                        record=result.model_dump(mode="json"),
                        commit_seq=tx.commit_seq,
                    )
                )
                return result
        finally:
            for lease in leases:
                if lease is not None:
                    await self.content.release(lease)

    async def read_in(
        self, view: ReadView | WriteTx, ctx: CallContext, seed_id: str, version: int | None = None
    ) -> SeedVersion:
        from sqlalchemy import select

        await require_namespace_in(view, ctx, "ego")
        statement = select(seed_versions.c.record).where(
            seed_versions.c.namespace_id == ctx.namespace_id,
            seed_versions.c.seed_id == seed_id,
        )
        if version is not None:
            statement = statement.where(seed_versions.c.version == version)
        value = await view.connection.scalar(
            statement.order_by(seed_versions.c.version.desc()).limit(1)
        )
        if value is None:
            raise DomainError("seed_not_found", "Seed version is unavailable")
        result = SeedVersion.model_validate(value)
        for source in result.sources:
            await self.materials.provenance_in(view, ctx, source)
        return result

    async def read(self, ctx: CallContext, seed_id: str, version: int | None = None) -> SeedVersion:
        async with self.authority.read(ctx) as view:
            return await self.read_in(view, ctx, seed_id, version)

    async def history(
        self, ctx: CallContext, seed_id: str, *, limit: int = 32
    ) -> tuple[SeedVersion, ...]:
        from sqlalchemy import select

        if not 1 <= limit <= 128:
            raise DomainError("seed_history_limit", "Seed history limit must be between 1 and 128")
        async with self.authority.read(ctx) as view:
            await self.read_in(view, ctx, seed_id)
            versions = (
                await view.connection.scalars(
                    select(seed_versions.c.version)
                    .where(
                        seed_versions.c.namespace_id == ctx.namespace_id,
                        seed_versions.c.seed_id == seed_id,
                    )
                    .order_by(seed_versions.c.version.desc())
                    .limit(limit)
                )
            ).all()
            return tuple([await self.read_in(view, ctx, seed_id, version) for version in versions])

    async def markdown(self, ctx: CallContext) -> str:
        records = await self.ego.ego_profile(ctx)
        lines = ["# 当前人格", ""]
        for record in records:
            if record.body is None:
                continue
            body = TypeAdapter(dict[str, JsonValue]).validate_python(
                record.body.model_dump(mode="json")
            )
            title = (
                body.get("facet")
                or body.get("domain")
                or body.get("title")
                or record.reference.kind
            )
            lines.extend([f"## {title}", ""])
            for field in ("description", "understanding", "narrative", "uncertainty"):
                value = body.get(field)
                if isinstance(value, str) and value:
                    lines.extend([value, ""])
            for field, label in (("contexts", "适用情境"), ("exceptions", "例外")):
                values = body.get(field)
                if isinstance(values, list) and values:
                    lines.extend([f"{label}：" + "；".join(str(value) for value in values), ""])
            lines.extend([f"修订：{record.reference.exact()}", ""])
            if record.source_roots:
                lines.extend(["来源：" + "、".join(record.source_roots), ""])
        return "\n".join(lines)
