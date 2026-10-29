"""Scope-effective retrieval parameters, with defaults owned by native Serde."""

from __future__ import annotations

import json
from dataclasses import dataclass

from nous_native import schemas_json
from nous_wave.contracts import CallContext
from nous_wave.contracts.retrieval_policy import RetrievalPolicy
from pydantic import JsonValue, TypeAdapter

from nous.platform.configuration.service import ConfigurationService

KEY = "retrieval"
_JSON = TypeAdapter(dict[str, JsonValue])


def defaults() -> RetrievalPolicy:
    native = json.loads(schemas_json())["query_options"]["properties"]
    value = {
        name: native[name]["default"] for name in ("lexical_weight", "dense_weight", "rrf_k", "vcp")
    }
    # Ghost vectors are request data and have no admission owner in this policy.
    del value["vcp"]["ghost_tags"]
    return RetrievalPolicy.model_validate(value)


@dataclass(frozen=True)
class ResolvedPolicy:
    value: RetrievalPolicy
    origin: str
    namespace_id: str
    subject_id: str

    def options(self, limit: int, diagnostics: str) -> dict[str, JsonValue]:
        value = self.value.model_dump(mode="json")
        if diagnostics == "none":
            value["vcp"]["include_trace"] = False
            value["vcp"]["include_fields"] = False
        elif diagnostics == "full":
            value["vcp"]["include_trace"] = True
        return _JSON.validate_python({**value, "limit": limit})

    def diagnostics(self) -> dict[str, JsonValue]:
        return _JSON.validate_python(
            {
                "key": KEY,
                "origin": self.origin,
                "namespace_id": self.namespace_id,
                "subject_id": self.subject_id,
                "effective": self.value.model_dump(mode="json"),
            }
        )


async def resolve(configuration: ConfigurationService, ctx: CallContext) -> ResolvedPolicy:
    effective = await configuration.effective(ctx, KEY)
    return ResolvedPolicy(
        RetrievalPolicy.model_validate(effective["value"]),
        str(effective["origin"]),
        ctx.namespace_id,
        ctx.require_subject(),
    )


def native_diagnostics(raw: str, level: str, budget: int) -> dict[str, JsonValue]:
    value = _JSON.validate_json(raw)
    if level == "none":
        return {}
    if level == "summary":
        value = {
            key: child
            for key, child in value.items()
            if key in {"mode", "allowed_rows", "elapsed_ms"}
        }
    size = len(json.dumps(value, ensure_ascii=False).encode("utf-8"))
    if size > budget:
        return {"truncated": True, "available_bytes": size, "budget_bytes": budget}
    return value
