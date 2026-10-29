"""Whole-key overrides, scoped inheritance and immutable change history."""

from dataclasses import dataclass
from typing import Literal

import sqlalchemy as sa
from nous_wave.contracts import CallContext
from nous_wave.contracts.configuration import ConfigurationCatalog, ConfigurationDefinition
from pydantic import BaseModel, TypeAdapter

from nous.platform.errors import NousError
from nous.platform.persistence import Authority, metadata

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


@dataclass(frozen=True)
class ConfigurationEntry:
    default: BaseModel
    instance_management: bool = False


class ConfigurationService:
    def __init__(self, authority: Authority, entries: dict[str, ConfigurationEntry]) -> None:
        self.authority = authority
        self._entries = dict(entries)
        self._defaults = {
            key: entry.default.model_dump(mode="json") for key, entry in self._entries.items()
        }

    def entry(self, key: str) -> ConfigurationEntry:
        try:
            return self._entries[key]
        except KeyError as error:
            raise NousError(
                "configuration_key_not_found",
                "Configuration key is not registered",
                status=404,
                context={"key": key},
            ) from error

    def describe(self, key: str) -> ConfigurationDefinition:
        entry = self.entry(key)
        return ConfigurationDefinition(
            key=key,
            value_schema=type(entry.default).model_json_schema(),
            default_value=self._defaults[key],
            write_access="instance_manage" if entry.instance_management else "manage",
        )

    def catalog(self) -> ConfigurationCatalog:
        return ConfigurationCatalog(
            items=tuple(self.describe(key) for key in sorted(self._entries))
        )

    def scopes(self, ctx: CallContext) -> tuple[str, ...]:
        scopes = ("instance", f"namespace:{ctx.namespace_id}")
        return (*scopes, f"subject:{ctx.subject_id}") if ctx.subject_id else scopes

    async def effective(self, ctx: CallContext, key: str) -> dict[str, object]:
        entry = self.entry(key)
        value = self._defaults[key]
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
        resolved = type(entry.default).model_validate(value).model_dump(mode="json")
        return {"key": key, "value": resolved, "origin": origin}

    async def history(self, ctx: CallContext, key: str, limit: int = 100) -> dict[str, object]:
        self.entry(key)
        if not 1 <= limit <= 1000:
            raise ValueError("Configuration history limit must be between 1 and 1000")
        async with self.authority.read(ctx) as view:
            rows = (
                await view.connection.execute(
                    sa.select(history)
                    .where(history.c.key == key, history.c.scope.in_(self.scopes(ctx)))
                    .order_by(history.c.commit_seq.desc())
                    .limit(limit)
                )
            ).mappings()
            return {"key": key, "items": [dict(row) for row in rows], "watermark": view.commit_seq}

    async def set(
        self,
        ctx: CallContext,
        key: str,
        value: object,
        operation_id: str,
        scope: Literal["instance", "namespace", "subject"] = "instance",
    ) -> dict[str, object]:
        entry = self.entry(key)
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
            else type(entry.default).model_validate(value).model_dump(mode="json")
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
