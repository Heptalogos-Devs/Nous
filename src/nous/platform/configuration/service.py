"""Whole-key overrides, scoped inheritance and immutable change history."""

import os
import tempfile
import tomllib
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Literal
from uuid import uuid4

import anyio
import sqlalchemy as sa
import tomlkit
from filelock import AsyncFileLock
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.configuration import ConfigurationCatalog, ConfigurationDefinition
from pydantic import BaseModel, JsonValue, TypeAdapter

from nous.platform.errors import NousError
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

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
    sa.Column("scope", sa.String, primary_key=True),
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("value", sa.JSON),
    sa.Column("previous_value", sa.JSON),
)


@dataclass(frozen=True)
class ConfigurationEntry:
    default: BaseModel
    instance_management: bool = False
    scopes: tuple[Literal["instance", "namespace"], ...] = ("instance", "namespace")
    validator: Callable[[BaseModel], None] | None = None
    schema: Callable[[], dict[str, JsonValue]] | None = None

    def parse(self, value: object) -> BaseModel:
        parsed = type(self.default).model_validate(value)
        if self.validator is not None:
            self.validator(parsed)
        return parsed


class ConfigurationValues:
    def __init__(self, entries: dict[str, ConfigurationEntry]) -> None:
        self._entries = dict(entries)
        self._defaults = {
            key: entry.parse(entry.default).model_dump(mode="json")
            for key, entry in self._entries.items()
        }

    @staticmethod
    def merge(base: dict[str, JsonValue], override: dict[str, JsonValue]) -> dict[str, JsonValue]:
        result = dict(base)
        for key, value in override.items():
            previous = result.get(key)
            result[key] = (
                ConfigurationValues.merge(previous, value)
                if isinstance(previous, dict) and isinstance(value, dict)
                else value
            )
        return result

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

    def scopes(self, ctx: CallContext) -> tuple[str, ...]:
        scopes = ("instance", f"namespace:{ctx.namespace_id}")
        return scopes

    async def effective_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext | None,
        key: str,
        *,
        scopes: tuple[str, ...] | None = None,
    ) -> dict[str, object]:
        entry = self.entry(key)
        if scopes is None:
            if ctx is None:
                raise ValueError("Scoped configuration requires a context")
            scopes = tuple(
                scope for scope in self.scopes(ctx) if scope.split(":", 1)[0] in entry.scopes
            )
        value = self._defaults[key]
        origin = "default"
        commit_seq = 0
        for scope in scopes:
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
                value, origin, commit_seq = (
                    self.merge(
                        TypeAdapter(dict[str, JsonValue]).validate_python(value),
                        TypeAdapter(dict[str, JsonValue]).validate_python(row["value"]),
                    ),
                    scope,
                    int(row["commit_seq"]),
                )
        resolved = entry.parse(value).model_dump(mode="json")
        return {"key": key, "value": resolved, "origin": origin, "commit_seq": commit_seq}

    async def policy_in[P: BaseModel](
        self, view: ReadView | WriteTx, ctx: CallContext, key: str, model: type[P]
    ) -> P:
        value = await self.effective_in(view, ctx, key)
        return model.model_validate(value["value"])


class ConfigurationService:
    def __init__(
        self, authority: Authority, values: ConfigurationValues, *, instance: Path
    ) -> None:
        self.authority = authority
        self.instance = instance.resolve()
        self.lock_path = self.instance / "run" / "configuration.lock"
        self.lock_path.parent.mkdir(parents=True, exist_ok=True)
        self.values = values
        self._entries = values._entries
        self._defaults = values._defaults

    @staticmethod
    def toml_value(value: JsonValue) -> JsonValue:
        if isinstance(value, dict):
            return {
                key: ConfigurationService.toml_value(item)
                for key, item in value.items()
                if item is not None
            }
        if isinstance(value, list):
            return [ConfigurationService.toml_value(item) for item in value]
        return value

    def path(self, ctx: CallContext, scope: Literal["instance", "namespace"]) -> Path:
        return (
            self.instance / "config.toml"
            if scope == "instance"
            else self.instance / "namespaces" / ctx.namespace_id / "config.toml"
        )

    def parse_document(self, content: str) -> dict[str, JsonValue]:
        raw = TypeAdapter(dict[str, JsonValue]).validate_python(tomllib.loads(content))
        result: dict[str, JsonValue] = {}

        def visit(value: dict[str, JsonValue], prefix: str = "") -> None:
            for name, item in value.items():
                key = prefix + name
                if key in self._entries:
                    result[key] = item
                elif isinstance(item, dict) and any(
                    candidate.startswith(key + ".") for candidate in self._entries
                ):
                    visit(item, key + ".")
                else:
                    raise DomainError(
                        "configuration_key_not_found",
                        "Configuration key is not registered",
                        key=key,
                    )

        visit(raw)
        return result

    def read_file(self, path: Path) -> dict[str, JsonValue]:
        try:
            return self.parse_document(path.read_text(encoding="utf-8"))
        except FileNotFoundError as error:
            raise DomainError(
                "configuration_file_not_found", "Configuration file does not exist", path=str(path)
            ) from error
        except (ValueError, TypeError) as error:
            raise DomainError("configuration_invalid", str(error), path=str(path)) from error

    @staticmethod
    def replace_file(path: Path, content: str) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        descriptor, name = tempfile.mkstemp(prefix=".config-", suffix=".toml", dir=path.parent)
        temporary = Path(name)
        try:
            with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
                stream.write(content)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)

    def update_file(self, path: Path, key: str, value: JsonValue) -> None:
        document = (
            tomlkit.parse(path.read_text(encoding="utf-8")) if path.exists() else tomlkit.document()
        )
        if value is None:
            if key in document:
                del document[key]
        else:
            document[key] = self.toml_value(value)
        self.replace_file(path, tomlkit.dumps(document))

    async def initialize(self) -> None:
        ctx = CallContext(
            caller_id="configuration", namespace_id="instance", credential_source_ids=None
        )
        path = self.path(ctx, "instance")
        async with AsyncFileLock(str(self.lock_path)):
            if not path.exists():
                async with self.authority.read() as view:
                    existing = await view.connection.scalar(
                        sa.select(configuration.c.key)
                        .where(configuration.c.scope == "instance")
                        .limit(1)
                    )
                if existing is not None:
                    raise DomainError(
                        "configuration_file_required",
                        "Write the current instance configuration to config.toml before starting",
                        path=str(path),
                    )
                document = tomlkit.document()
                for key, entry in self._entries.items():
                    if "instance" in entry.scopes:
                        document[key] = self.toml_value(self._defaults[key])
                await anyio.to_thread.run_sync(self.replace_file, path, tomlkit.dumps(document))
        await self.reload(ctx, "instance")
        directory = self.instance / "namespaces"
        if directory.is_dir():
            for file in sorted(directory.glob("*/config.toml")):
                scoped = ctx.model_copy(update={"namespace_id": file.parent.name})
                await self.reload(scoped, "namespace")

    async def load(
        self, ctx: CallContext, scope: Literal["instance", "namespace"] = "instance"
    ) -> dict[str, JsonValue]:
        return await anyio.to_thread.run_sync(self.read_file, self.path(ctx, scope))

    async def validate(
        self,
        ctx: CallContext,
        values: dict[str, JsonValue],
        scope: Literal["instance", "namespace"] = "instance",
    ) -> dict[str, JsonValue]:
        checked: dict[str, JsonValue] = {}
        inherited_scopes = () if scope == "instance" else ("instance",)
        async with self.authority.read(ctx) as view:
            for key, value in values.items():
                entry = self.entry(key)
                if scope not in entry.scopes:
                    raise DomainError(
                        "configuration_scope_invalid",
                        "Configuration does not support this scope",
                        key=key,
                        scope=scope,
                    )
                base = (await self.effective_in(view, ctx, key, scopes=inherited_scopes))["value"]
                merged = ConfigurationValues.merge(
                    TypeAdapter(dict[str, JsonValue]).validate_python(base),
                    TypeAdapter(dict[str, JsonValue]).validate_python(value),
                )
                checked[key] = entry.parse(merged).model_dump(mode="json")
        return checked

    async def _publish_document_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        values: dict[str, JsonValue],
        scope: Literal["instance", "namespace"],
    ) -> list[str]:
        target = "instance" if scope == "instance" else "namespace:" + ctx.namespace_id
        current = {
            str(row.key): row.value
            for row in (
                await tx.connection.execute(
                    sa.select(configuration.c.key, configuration.c.value).where(
                        configuration.c.scope == target
                    )
                )
            ).all()
        }
        changed = []
        for key in sorted(set(values) | set(current)):
            if values.get(key) != current.get(key):
                await self._publish_in(tx, ctx, key, values.get(key), scope)
                changed.append(key)
        return changed

    async def reload(
        self,
        ctx: CallContext,
        scope: Literal["instance", "namespace"] = "instance",
        operation_id: str | None = None,
    ) -> dict[str, object]:
        target = "instance" if scope == "instance" else "namespace:" + ctx.namespace_id
        async with self.authority.write(
            ctx,
            operation_id or str(uuid4()),
            operation_kind="configuration.reload",
            scope=target,
            payload={"scope": target},
        ) as tx:
            if tx.replayed:
                return TypeAdapter(dict[str, object]).validate_python(tx.replay_result)
            async with AsyncFileLock(str(self.lock_path)):
                values = await self.load(ctx, scope)
                await self.validate(ctx, values, scope)
                changed = await self._publish_document_in(tx, ctx, values, scope)
                return tx.result(
                    {
                        "scope": target,
                        "path": str(self.path(ctx, scope)),
                        "commit_seq": tx.commit_seq,
                        "changed": changed,
                    }
                )

    async def replace(
        self,
        ctx: CallContext,
        content: str,
        operation_id: str,
        scope: Literal["instance", "namespace"] = "instance",
    ) -> dict[str, object]:
        values = self.parse_document(content)
        target = "instance" if scope == "instance" else "namespace:" + ctx.namespace_id
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="configuration.replace",
            scope=target,
            payload={"content": content},
        ) as tx:
            if tx.replayed:
                return TypeAdapter(dict[str, object]).validate_python(tx.replay_result)
            async with AsyncFileLock(str(self.lock_path)):
                await self.validate(ctx, values, scope)
                await anyio.to_thread.run_sync(self.replace_file, self.path(ctx, scope), content)
                changed = await self._publish_document_in(tx, ctx, values, scope)
                return tx.result(
                    {
                        "scope": target,
                        "path": str(self.path(ctx, scope)),
                        "commit_seq": tx.commit_seq,
                        "changed": changed,
                    }
                )

    async def export(
        self, ctx: CallContext, scope: Literal["instance", "namespace"] = "instance"
    ) -> str:
        return await anyio.to_thread.run_sync(self.path(ctx, scope).read_text, "utf-8")

    def entry(self, key: str) -> ConfigurationEntry:
        return self.values.entry(key)

    def describe(self, key: str) -> ConfigurationDefinition:
        entry = self.entry(key)
        return ConfigurationDefinition(
            key=key,
            value_schema=entry.schema()
            if entry.schema
            else type(entry.default).model_json_schema(),
            default_value=self._defaults[key],
            write_access="instance_manage" if entry.instance_management else "manage",
            scopes=entry.scopes,
        )

    def catalog(self) -> ConfigurationCatalog:
        return ConfigurationCatalog(
            items=tuple(self.describe(key) for key in sorted(self._entries))
        )

    def scopes(self, ctx: CallContext) -> tuple[str, ...]:
        return self.values.scopes(ctx)

    async def effective(self, ctx: CallContext, key: str) -> dict[str, object]:
        entry = self.entry(key)
        scopes = tuple(
            scope for scope in self.scopes(ctx) if scope.split(":", 1)[0] in entry.scopes
        )
        return await self._effective(key, scopes, ctx)

    async def effective_instance(self, key: str) -> dict[str, object]:
        entry = self.entry(key)
        if entry.scopes != ("instance",):
            raise ValueError("Instance resource resolution requires an instance-only definition")
        return await self._effective(key, ("instance",), None)

    async def _effective(
        self, key: str, scopes: tuple[str, ...], ctx: CallContext | None
    ) -> dict[str, object]:
        async with self.authority.read(ctx) as view:
            return await self.effective_in(view, ctx, key, scopes=scopes)

    async def effective_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext | None,
        key: str,
        *,
        scopes: tuple[str, ...] | None = None,
    ) -> dict[str, object]:
        return await self.values.effective_in(view, ctx, key, scopes=scopes)

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
        scope: Literal["instance", "namespace"] = "instance",
    ) -> dict[str, object]:
        entry = self.entry(key)
        if scope not in entry.scopes:
            raise NousError(
                "configuration_scope_invalid",
                "Configuration does not support the selected scope",
                context={"key": key, "scope": scope, "allowed_scopes": list(entry.scopes)},
            )
        target = "instance" if scope == "instance" else f"{scope}:{ctx.namespace_id}"
        parsed = (
            None
            if value is None
            else TypeAdapter(dict[str, JsonValue]).validate_python(
                self.toml_value(TypeAdapter(dict[str, JsonValue]).validate_python(value))
            )
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
            return tx.result(await self.set_in(tx, ctx, key, parsed, scope))

    async def set_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        key: str,
        value: object,
        scope: Literal["instance", "namespace"] = "namespace",
    ) -> dict[str, object]:
        raw = None if value is None else TypeAdapter(dict[str, JsonValue]).validate_python(value)
        entry = self.entry(key)
        if scope not in entry.scopes:
            raise DomainError(
                "configuration_scope_invalid",
                "Configuration does not support this scope",
                key=key,
                scope=scope,
            )
        inherited = await self.effective_in(
            tx, ctx, key, scopes=() if scope == "instance" else ("instance",)
        )
        if raw is not None:
            entry.parse(
                ConfigurationValues.merge(
                    TypeAdapter(dict[str, JsonValue]).validate_python(inherited["value"]), raw
                )
            )
        async with AsyncFileLock(str(self.lock_path)):
            await anyio.to_thread.run_sync(self.update_file, self.path(ctx, scope), key, raw)
            return await self._publish_in(tx, ctx, key, raw, scope)

    async def _publish_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        key: str,
        value: object,
        scope: Literal["instance", "namespace"] = "namespace",
    ) -> dict[str, object]:
        if tx.authority is not self.authority or tx.replayed:
            raise ValueError("Configuration changes require the owning live transaction")
        entry = self.entry(key)
        if scope not in entry.scopes:
            raise NousError(
                "configuration_scope_invalid",
                "Configuration does not support the selected scope",
                context={"key": key, "scope": scope},
            )
        target = "instance" if scope == "instance" else f"{scope}:{ctx.namespace_id}"
        parsed = (
            None
            if value is None
            else TypeAdapter(dict[str, JsonValue]).validate_python(
                self.toml_value(TypeAdapter(dict[str, JsonValue]).validate_python(value))
            )
        )
        if (
            key == "namespace.quota"
            and parsed is not None
            and (parsed.get("max_storage_bytes") is not None)
            and (tx.storage_baseline is None)
            and (self.authority.namespace_limits is not None)
        ):
            usage = await self.authority.namespace_limits.usage_in(tx, ctx)
            tx.storage_baseline = int(str(usage["storage_bytes"]))
        if parsed is not None:
            inherited = await self.effective_in(
                tx, ctx, key, scopes=() if scope == "instance" else ("instance",)
            )
            entry.parse(
                ConfigurationValues.merge(
                    TypeAdapter(dict[str, JsonValue]).validate_python(inherited["value"]), parsed
                )
            )
        applied_scopes = self.scopes(ctx)
        target_scopes = applied_scopes[: applied_scopes.index(target) + 1]
        previous = await self.effective_in(tx, ctx, key, scopes=target_scopes)
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
                scope=target,
                key=key,
                value=parsed,
                previous_value=previous["value"],
                commit_seq=tx.commit_seq,
            )
        )
        return {"key": key, "scope": target, "value": parsed, "commit_seq": tx.commit_seq}

    async def restore_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        key: str,
        commit_seq: int | None = None,
        scope: Literal["instance", "namespace"] = "namespace",
    ) -> dict[str, object]:
        """Publish a retained configuration value as a new revision at the chosen scope."""
        target = "instance" if scope == "instance" else f"{scope}:{ctx.namespace_id}"
        statement = sa.select(history).where(history.c.key == key, history.c.scope == target)
        if commit_seq is not None:
            statement = statement.where(history.c.commit_seq == commit_seq)
        row = (
            (await tx.connection.execute(statement.order_by(history.c.commit_seq.desc()).limit(1)))
            .mappings()
            .first()
        )
        if row is None:
            raise NousError(
                "configuration_revision_not_found",
                "Configuration history is unavailable",
                status=404,
            )
        if commit_seq is not None:
            value = row["value"]
        else:
            value = row["previous_value"]
        return await self.set_in(tx, ctx, key, value, scope)

    async def policy_in[P: BaseModel](
        self, view: ReadView | WriteTx, ctx: CallContext, key: str, model: type[P]
    ) -> P:
        return await self.values.policy_in(view, ctx, key, model)

    async def policy[P: BaseModel](self, ctx: CallContext, key: str, model: type[P]) -> P:
        async with self.authority.read(ctx) as view:
            return await self.values.policy_in(view, ctx, key, model)
