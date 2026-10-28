"""Versioned Jinja templates with immutable call snapshots."""

import hashlib
import re
from importlib.resources import files
from importlib.resources.abc import Traversable

import sqlalchemy as sa
from jinja2 import StrictUndefined, meta
from jinja2.sandbox import ImmutableSandboxedEnvironment
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
        self.environment = ImmutableSandboxedEnvironment(
            undefined=StrictUndefined, autoescape=False
        )

    @staticmethod
    def bundled_ids() -> set[str]:
        def visit(root: Traversable, prefix: str = "") -> set[str]:
            result: set[str] = set()
            for entry in root.iterdir():
                if entry.is_dir():
                    result.update(visit(entry, prefix + entry.name + "."))
                elif entry.name.endswith((".md", ".jinja")):
                    result.add(prefix + entry.name.rsplit(".", 1)[0])
            return result

        return visit(files("nous").joinpath("resources/prompts"))

    def describe(self, record: dict[str, object]) -> dict[str, object]:
        template = str(record["template"])
        return {
            **record,
            "source": "bundled" if record["revision"] == 0 else "instance",
            "variable_names": sorted(
                meta.find_undeclared_variables(self.environment.parse(template))
            ),
            "template_hash": hashlib.sha256(template.encode()).hexdigest(),
        }

    @staticmethod
    def bundled(prompt_id: str) -> str:
        if not re.fullmatch(r"[a-zA-Z0-9_-]+(?:\.[a-zA-Z0-9_-]+)*", prompt_id):
            raise KeyError("Unknown prompt resource")
        root = files("nous").joinpath("resources/prompts")
        parts = prompt_id.split(".")
        for extension in ("jinja", "md"):
            path = root.joinpath(*parts[:-1], parts[-1] + "." + extension)
            if path.is_file():
                return path.read_text(encoding="utf-8")
        raise KeyError(prompt_id)

    async def list(self, ctx: CallContext) -> list[dict[str, object]]:
        async with self.authority.read(ctx) as view:
            ids = set((await view.connection.execute(sa.select(prompts.c.id).distinct())).scalars())
        return [await self.get(ctx, prompt_id) for prompt_id in sorted(ids | self.bundled_ids())]

    async def history(self, ctx: CallContext, prompt_id: str) -> list[dict[str, object]]:
        async with self.authority.read(ctx) as view:
            rows = (
                await view.connection.execute(
                    sa.select(prompts)
                    .where(prompts.c.id == prompt_id)
                    .order_by(prompts.c.revision.desc())
                )
            ).mappings()
            result = [self.describe(dict(row)) for row in rows]
        if prompt_id in self.bundled_ids():
            result.append(
                self.describe(
                    {
                        "id": prompt_id,
                        "revision": 0,
                        "template": self.bundled(prompt_id),
                        "active": not any(row["active"] for row in result),
                    }
                )
            )
        return result

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
            if found is None:
                if revision != 0:
                    raise KeyError("Prompt revision does not exist")
                self.bundled(prompt_id)
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
            return self.describe(
                {
                    "id": prompt_id,
                    "revision": 0,
                    "template": self.bundled(prompt_id),
                    "active": True,
                }
            )
        return self.describe(dict(row))

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
        return self.preview(record, variables)

    def preview(self, record: dict[str, object], variables: dict[str, object]) -> dict[str, object]:
        rendered = self.environment.from_string(str(record["template"])).render(variables)
        return {
            **self.describe(record),
            "variables": variables,
            "rendered": rendered,
            "hash": hashlib.sha256(rendered.encode()).hexdigest(),
            "variable_names": sorted(
                meta.find_undeclared_variables(self.environment.parse(str(record["template"])))
            ),
        }
