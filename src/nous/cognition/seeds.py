"""Creator documents use the shared typed proposal and exact revision adoption path."""

from nous_sdk.contracts import CallContext, DomainError, FreezeCatalog
from nous_sdk.contracts.ego_model import InterpretSeed, SeedVersion
from nous_sdk.contracts.maintenance import MaintenanceRun, MaintenanceSubmission, StartMaintenance
from pydantic import JsonValue, TypeAdapter

from nous.cognition.ego_model import EgoService
from nous.cognition.records import CognitiveRecords
from nous.identity.access import require_namespace_in
from nous.imports.service import ImportService
from nous.maintenance.service import MaintenanceService
from nous.platform.persistence import Authority


class SeedService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        maintenance: MaintenanceService,
        imports: ImportService,
        *,
        ego: EgoService,
    ) -> None:
        self.ego = ego
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
                        operation_id=frozen.operation_id,
                        operation_kind="imports.freeze",
                        scope=ctx.namespace_id,
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

    async def read(self, ctx: CallContext, seed_id: str) -> SeedVersion:
        run = await self.maintenance.read(ctx, seed_id)
        if run.command.kind != "seed" or run.command.source_catalog_id is None:
            raise DomainError("seed_not_found", "The requested Seed source is unavailable")
        from .growth_sources import from_units_in, read_in

        async with self.authority.read(ctx) as view:
            catalog = await self.imports.read_catalog_in(view, ctx, run.command.source_catalog_id)
            sources = await read_in(
                self.cognition,
                view,
                ctx,
                await from_units_in(self.cognition, view, ctx, catalog.units),
            )
        return SeedVersion(
            id=run.id,
            previous_seed_id=run.command.previous_seed_id,
            instruction=run.command.instruction,
            created_at=run.created_at,
            sources=sources,
            adopted=run.review.adopted if run.review else (),
            review=run.review.decision if run.review else None,
        )

    async def history(
        self, ctx: CallContext, seed_id: str, *, limit: int = 32
    ) -> tuple[SeedVersion, ...]:
        if not 1 <= limit <= 128:
            raise DomainError(
                "seed_history_limit", "Seed history requires a limit between 1 and 128"
            )
        versions: list[SeedVersion] = []
        current: str | None = seed_id
        while current is not None and len(versions) < limit:
            version = await self.read(ctx, current)
            versions.append(version)
            current = version.previous_seed_id
        return tuple(versions)

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
