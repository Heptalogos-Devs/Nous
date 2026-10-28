"""Whole-key overrides, scoped inheritance and immutable change history."""

from typing import Literal

import sqlalchemy as sa
from nous_wave.amemorix.contracts import AMemorixPolicy
from nous_wave.contracts import CallContext
from pydantic import BaseModel, TypeAdapter

from nous.platform.persistence import Authority, metadata

from .models import ModelConfiguration

configuration = sa.Table(
    "nous_configuration",
    metadata,
    sa.Column("scope", sa.String, primary_key=True),
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("value", sa.JSON, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
)
history = sa.Table(
    "nous_configuration_history",
    metadata,
    sa.Column("commit_seq", sa.Integer, primary_key=True),
    sa.Column("scope", sa.String, nullable=False),
    sa.Column("key", sa.String, nullable=False),
    sa.Column("value", sa.JSON),
)


class ConfigurationService:
    schemas: dict[str, type[BaseModel]] = {
        "models": ModelConfiguration,
        "integrations.amemorix": AMemorixPolicy,
    }

    def __init__(self, authority: Authority, defaults: dict[str, object]) -> None:
        self.authority = authority
        self.defaults = defaults

    def scopes(self, ctx: CallContext) -> tuple[str, ...]:
        scopes = ("instance", f"namespace:{ctx.namespace_id}")
        return (*scopes, f"subject:{ctx.subject_id}") if ctx.subject_id else scopes

    async def effective(self, ctx: CallContext, key: str) -> dict[str, object]:
        if key not in self.schemas:
            raise KeyError(key)
        value = self.defaults[key]
        origin = "default"
        async with self.authority.read(ctx) as view:
            for scope in self.scopes(ctx):
                row = (
                    (
                        await view.connection.execute(
                            sa.select(configuration).where(
                                configuration.c.scope == scope, configuration.c.key == key
                            )
                        )
                    )
                    .mappings()
                    .first()
                )
                if row:
                    value, origin = row["value"], scope
        return {"key": key, "value": value, "origin": origin}

    async def set(
        self,
        ctx: CallContext,
        key: str,
        value: object,
        operation_id: str,
        scope: Literal["instance", "namespace", "subject"] = "instance",
    ) -> dict[str, object]:
        if key not in self.schemas:
            raise KeyError(key)
        if scope == "subject" and ctx.subject_id is None:
            raise ValueError("Subject scope requires a subject")
        target = (
            "instance"
            if scope == "instance"
            else f"{scope}:{ctx.namespace_id if scope == 'namespace' else ctx.subject_id}"
        )
        parsed = (
            None
            if value is None
            else self.schemas[key].model_validate(value).model_dump(mode="json")
        )
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="configuration.set",
            scope=target,
            payload={"key": key, "value": parsed},
        ) as tx:
            if tx.replayed:
                assert tx.receipt is not None
                return TypeAdapter(dict[str, object]).validate_python(tx.receipt.result)
            await tx.connection.execute(
                configuration.delete().where(
                    configuration.c.scope == target, configuration.c.key == key
                )
            )
            if parsed is not None:
                await tx.connection.execute(
                    configuration.insert().values(
                        scope=target, key=key, value=parsed, commit_seq=tx.commit_seq
                    )
                )
            await tx.connection.execute(
                history.insert().values(
                    scope=target, key=key, value=parsed, commit_seq=tx.commit_seq
                )
            )
            return tx.result(
                {"key": key, "scope": target, "value": parsed, "commit_seq": tx.commit_seq}
            )
