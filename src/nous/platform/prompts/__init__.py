"""Versioned Jinja templates with immutable call snapshots."""

import hashlib
from importlib.resources import files

import sqlalchemy as sa
from jinja2 import Environment, StrictUndefined, meta
from nous_wave.contracts import CallContext
from pydantic import TypeAdapter

from nous.platform.persistence import Authority, metadata

prompts = sa.Table(
    "nous_prompts",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("revision", sa.Integer, primary_key=True),
    sa.Column("template", sa.Text, nullable=False),
    sa.Column("active", sa.Boolean, nullable=False),
)


class PromptService:
    def __init__(self, authority: Authority) -> None:
        self.authority = authority
        self.environment = Environment(undefined=StrictUndefined, autoescape=False)

    async def list(self, ctx: CallContext) -> list[dict[str, object]]:
        async with self.authority.read(ctx) as view:
            ids = set((await view.connection.execute(sa.select(prompts.c.id).distinct())).scalars())
        return [await self.get(ctx, prompt_id) for prompt_id in sorted(ids | {"formation"})]

    async def history(self, ctx: CallContext, prompt_id: str) -> list[dict[str, object]]:
        async with self.authority.read(ctx) as view:
            rows = (
                await view.connection.execute(
                    sa.select(prompts)
                    .where(prompts.c.id == prompt_id)
                    .order_by(prompts.c.revision.desc())
                )
            ).mappings()
            return [dict(row) for row in rows]

    async def activate(
        self, ctx: CallContext, prompt_id: str, revision: int, operation_id: str
    ) -> dict[str, object]:
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="prompt.activate",
            payload={"id": prompt_id, "revision": revision},
        ) as tx:
            if tx.replayed:
                assert tx.receipt is not None
                return TypeAdapter(dict[str, object]).validate_python(tx.receipt.result)
            found = await tx.connection.scalar(
                sa.select(prompts.c.revision).where(
                    prompts.c.id == prompt_id, prompts.c.revision == revision
                )
            )
            if found is None and not (revision == 0 and prompt_id == "formation"):
                raise KeyError("Prompt revision does not exist")
            await tx.connection.execute(
                prompts.update().where(prompts.c.id == prompt_id).values(active=False)
            )
            if revision:
                await tx.connection.execute(
                    prompts.update()
                    .where(prompts.c.id == prompt_id, prompts.c.revision == revision)
                    .values(active=True)
                )
            return tx.result({"id": prompt_id, "revision": revision})

    async def get(self, ctx: CallContext, prompt_id: str = "formation") -> dict[str, object]:
        async with self.authority.read(ctx) as view:
            row = (
                (
                    await view.connection.execute(
                        sa.select(prompts).where(
                            prompts.c.id == prompt_id, prompts.c.active.is_(True)
                        )
                    )
                )
                .mappings()
                .first()
            )
        if row is None:
            if prompt_id != "formation":
                raise KeyError(prompt_id)
            return {
                "id": prompt_id,
                "revision": 0,
                "template": files("nous")
                .joinpath("resources/prompts/formation.jinja")
                .read_text(encoding="utf-8"),
            }
        return dict(row)

    async def save(
        self, ctx: CallContext, prompt_id: str, template: str, operation_id: str
    ) -> dict[str, object]:
        self.environment.parse(template)
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="prompt.save",
            payload={"id": prompt_id, "template": template},
        ) as tx:
            if tx.replayed:
                assert tx.receipt is not None
                return TypeAdapter(dict[str, object]).validate_python(tx.receipt.result)
            revision = (
                int(
                    await tx.connection.scalar(
                        sa.select(sa.func.max(prompts.c.revision)).where(prompts.c.id == prompt_id)
                    )
                    or 0
                )
                + 1
            )
            await tx.connection.execute(
                prompts.update().where(prompts.c.id == prompt_id).values(active=False)
            )
            await tx.connection.execute(
                prompts.insert().values(
                    id=prompt_id, revision=revision, template=template, active=True
                )
            )
            return tx.result({"id": prompt_id, "revision": revision, "template": template})

    async def render(
        self, ctx: CallContext, variables: dict[str, object], prompt_id: str = "formation"
    ) -> dict[str, object]:
        record = await self.get(ctx, prompt_id)
        rendered = self.environment.from_string(str(record["template"])).render(variables)
        return {
            **record,
            "variables": variables,
            "rendered": rendered,
            "hash": hashlib.sha256(rendered.encode()).hexdigest(),
            "variable_names": sorted(
                meta.find_undeclared_variables(self.environment.parse(str(record["template"])))
            ),
        }
