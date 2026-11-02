"""A-Memorix namespace fields select native policies and trusted resources."""

from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING

import sqlalchemy as sa
from nous_sdk.contracts import CallContext
from nous_sdk.contracts.namespace_policy import FormationPolicy, NamespaceFeatures, NamespaceQuota
from pydantic import JsonValue, TypeAdapter

from nous.identity.access import require_namespace_in
from nous.identity.service import require_management_scope
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.configuration.service import configuration
from nous.platform.errors import NousError
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from nous.application import Application

_JSON = TypeAdapter(dict[str, JsonValue])
_OBJECT = TypeAdapter(dict[str, object])
_KEYS = ("models", "namespace.features", "formation", "namespace.quota")


def _provider(resources: ModelConfiguration, role: str) -> dict[str, JsonValue] | None:
    assigned = resources.roles.get(role)
    if assigned is None:
        return None
    route_id = assigned.preset
    model = resources.catalog[resources.presets[route_id].model]
    return {"provider_id": model.provider, "model_id": route_id, "secret_ref": ""}


async def _read_in(
    app: Application, view: ReadView | WriteTx, ctx: CallContext
) -> dict[str, JsonValue]:
    await require_namespace_in(view, ctx, purpose="management")
    scope = ctx.model_copy(update={})
    values = {
        key: (await app.configuration.effective_in(view, scope, key))["value"] for key in _KEYS
    }
    models = ModelConfiguration.model_validate(values["models"])
    version = await view.connection.scalar(
        sa.select(sa.func.max(configuration.c.commit_seq)).where(
            configuration.c.scope == f"namespace:{ctx.namespace_id}", configuration.c.key.in_(_KEYS)
        )
    )
    native = {"provider_id": "native", "model_id": "", "secret_ref": ""}
    return _JSON.validate_python(
        {
            "config": {
                "embedding": _provider(models, "embedding"),
                "llm": _provider(models, "formation"),
                "identity_resolver": native,
                "message_source": native,
                "features": values["namespace.features"],
                "relation_extraction": values["formation"],
            },
            "quota": values["namespace.quota"],
            "config_version": int(version or 1),
            "resource_usage": await app.namespace_limits.usage_in(view, ctx),
        }
    )


async def read_namespace_config(app: Application, ctx: CallContext) -> dict[str, JsonValue]:
    async with app.authority.read(ctx) as view:
        return await _read_in(app, view, ctx)


def _select_route(resources: ModelConfiguration, raw: object, role: str) -> str:
    reference = _OBJECT.validate_python(raw)
    if set(reference).difference({"provider_id", "model_id", "secret_ref"}):
        raise ValueError("Provider reference accepts only registered resource identifiers")
    provider_id = str(reference.get("provider_id") or "")
    provider = resources.providers.get(provider_id)
    if provider is None:
        raise NousError(
            "provider_not_found",
            "Provider is not registered",
            status=404,
            context={"provider_id": provider_id},
        )
    secret = str(reference.get("secret_ref") or "")
    registered = {
        provider.credential_env,
        str(provider.credential_file) if provider.credential_file else "",
    }
    if secret and secret not in registered:
        raise NousError(
            "provider_secret_reference_invalid",
            "Secret reference does not match the registered provider",
            status=403,
        )
    model_id = str(reference.get("model_id") or "")
    capability = "embedding" if role == "embedding" else "text"
    candidates = [
        key
        for key, route in resources.presets.items()
        if resources.catalog[route.model].provider == provider_id
        and capability in resources.catalog[route.model].capabilities
        and (not model_id or key == model_id or route.model == model_id)
    ]
    if len(candidates) != 1:
        raise NousError(
            "model_resource_not_found" if not candidates else "model_resource_ambiguous",
            "Select one registered compatible model route",
            status=404 if not candidates else 409,
            context=_JSON.validate_python(
                {"provider_id": provider_id, "model_id": model_id, "routes": candidates}
            ),
        )
    return candidates[0]


async def apply_namespace_config_in(
    app: Application,
    tx: WriteTx,
    ctx: CallContext,
    config: Mapping[str, JsonValue],
    *,
    quota: Mapping[str, JsonValue] | None = None,
    expected_config_version: int | None = None,
) -> dict[str, JsonValue]:
    require_management_scope(ctx)
    before = await _read_in(app, tx, ctx)
    if expected_config_version is not None and before["config_version"] != expected_config_version:
        raise NousError(
            "namespace_config_conflict",
            "Namespace configuration version changed",
            status=409,
            context={
                "expected_config_version": expected_config_version,
                "config_version": before["config_version"],
            },
        )
    allowed = {
        "embedding",
        "llm",
        "identity_resolver",
        "message_source",
        "features",
        "relation_extraction",
    }
    if set(config).difference(allowed):
        raise ValueError("Unknown namespace configuration field")
    for name in ("identity_resolver", "message_source"):
        if config.get(name) is not None:
            ref = _OBJECT.validate_python(config[name])
            if (
                set(ref).difference({"provider_id", "model_id", "secret_ref"})
                or ref.get("provider_id") != "native"
                or ref.get("model_id")
                or ref.get("secret_ref")
            ):
                raise NousError(
                    "provider_not_found",
                    "Only the registered native provider is available",
                    status=404,
                    context={"provider": name, "provider_id": str(ref.get("provider_id") or "")},
                )
    scope = ctx.model_copy(update={})
    updates: dict[str, object] = {}
    if any(config.get(name) is not None for name in ("embedding", "llm")):
        trusted = ModelConfiguration.model_validate(
            (await app.configuration.effective_in(tx, scope, "models", scopes=("instance",)))[
                "value"
            ]
        )
        current = ModelConfiguration.model_validate(
            (await app.configuration.effective_in(tx, scope, "models"))["value"]
        )
        roles = dict(current.roles)
        for name, role in (("embedding", "embedding"), ("llm", "formation")):
            if config.get(name) is None:
                continue
            route = _select_route(trusted, config[name], role)
            for key, assigned in tuple(roles.items()):
                if (assigned.purpose == "embedding") == (role == "embedding"):
                    roles[key] = assigned.model_copy(update={"routes": (route,)})
        # Use the existing models key; only trusted instance resources enter this override.
        updates["models"] = trusted.model_copy(update={"roles": roles}).model_dump(mode="json")
    for field, key, model in (
        ("features", "namespace.features", NamespaceFeatures),
        ("relation_extraction", "formation", FormationPolicy),
    ):
        if config.get(field) is not None:
            raw = _OBJECT.validate_python(config[field])
            if field == "relation_extraction":
                if not raw.get("profile"):
                    raw.pop("profile", None)
                elif raw["profile"] != "native":
                    raise NousError(
                        "formation_profile_not_found",
                        "Formation profile is not registered",
                        status=404,
                        context={"profile": str(raw["profile"])},
                    )
                if raw.get("enabled") is False and "default_enabled" not in raw:
                    raw["default_enabled"] = False
            current = _OBJECT.validate_python(
                (await app.configuration.effective_in(tx, scope, key))["value"]
            )
            updates[key] = model.model_validate({**current, **raw}).model_dump(mode="json")
    if quota is not None:
        updates["namespace.quota"] = NamespaceQuota.model_validate(quota).model_dump(mode="json")
    if not updates:
        updates["namespace.features"] = (
            await app.configuration.effective_in(tx, scope, "namespace.features")
        )["value"]
    for key, value in updates.items():
        await app.configuration.set_in(tx, scope, key, value, "namespace")
    return await _read_in(app, tx, ctx)


async def update_namespace_config(
    app: Application,
    ctx: CallContext,
    config: Mapping[str, JsonValue],
    operation_id: str,
    expected_config_version: int | None = None,
    *,
    quota: Mapping[str, JsonValue] | None = None,
) -> dict[str, JsonValue]:
    async with app.authority.write(
        ctx,
        operation_id,
        operation_kind="namespace.configure",
        scope=ctx.namespace_id,
        payload={
            "config": config,
            "quota": quota,
            "expected_config_version": expected_config_version,
        },
    ) as tx:
        if tx.replayed:
            return _JSON.validate_python(tx.replay_result)
        return tx.result(
            await apply_namespace_config_in(
                app, tx, ctx, config, quota=quota, expected_config_version=expected_config_version
            )
        )


async def create_namespace_configured(
    app: Application,
    ctx: CallContext,
    namespace_id: str,
    config: Mapping[str, JsonValue],
    quota: Mapping[str, JsonValue] | None,
    operation_id: str,
):
    from nous_sdk.contracts.identity import Namespace

    target = ctx.model_copy(update={"namespace_id": namespace_id})
    require_management_scope(target)
    async with app.authority.write(
        target,
        operation_id,
        operation_kind="namespace.create_configured",
        scope=namespace_id,
        payload={"namespace_id": namespace_id, "config": config, "quota": quota},
    ) as tx:
        if tx.replayed:
            return Namespace.model_validate(tx.replay_result)
        await app.identity.create_namespace_in(tx, target, namespace_id, namespace_id)
        await apply_namespace_config_in(app, tx, target, config, quota=quota)
        subject = await app.identity.create_subject_in(tx, target, "memory", ("memory",))
        namespace = await app.identity.bind_memory_subject_in(tx, target, subject.id)
        return tx.result(namespace)
