"""Resolve source selectors into OR clauses whose conjunction preserves scope."""

import base64
import binascii
from dataclasses import dataclass

from fastapi import Request
from nous_wave.contracts import CallContext, CognitiveRef, DomainError, MediaReference
from nous_wave.contracts.context import CognitiveTarget, ContextTarget, MediaTarget, WorkContext
from nous_wave.source_scope import (
    CognitionSource,
    MediaSource,
    ObservationSource,
    OriginalMedia,
    SourceReadScope,
    WorkContextSource,
)
from pydantic import TypeAdapter, ValidationError
from sqlalchemy import select

from nous.application import Application
from nous.context.models import contexts
from nous.identity.service import require_subject_in
from nous.material.service import SourceRecord, observations, regions

_SCOPES = TypeAdapter(tuple[SourceReadScope, ...])
_SOURCE = TypeAdapter(SourceRecord)


@dataclass(frozen=True)
class ResolvedSourceScope:
    allowed_source_ids: tuple[str, ...] | None
    source_constraints: tuple[tuple[str, ...], ...] = ()


def resolved_scope(clauses: list[set[str]]) -> ResolvedSourceScope:
    groups = tuple(tuple(sorted(group)) for group in clauses)
    if any(not group for group in groups):
        return ResolvedSourceScope((), groups)
    return ResolvedSourceScope(
        tuple(sorted({token for group in groups for token in group})), groups
    )


def union_selectors(alternatives: list[list[set[str]]]) -> list[set[str]]:
    """Distribute selector alternatives without losing region/event conjunctions."""
    if not alternatives:
        return [set()]
    result = alternatives[0]
    for alternative in alternatives[1:]:
        combined = {frozenset(left | right) for left in result for right in alternative}
        ordered = sorted(combined, key=lambda group: (len(group), tuple(sorted(group))))
        reduced: list[set[str]] = []
        for group in ordered:
            if not any(prior.issubset(group) for prior in reduced):
                reduced.append(set(group))
            if len(reduced) > 1024:
                raise DomainError(
                    "source_scope_complex",
                    "Source selectors require too many independent constraints",
                )
        result = reduced
    return result


async def resolve_scope(
    request: Request,
    namespace_id: str,
    subject_id: str | None,
    base_scope: tuple[str, ...] | None = None,
) -> ResolvedSourceScope:
    header = request.headers.get("x-nous-source-read")
    if header is None:
        return ResolvedSourceScope(base_scope, (base_scope,) if base_scope is not None else ())
    if len(header) > 65536:
        raise DomainError("source_scope_invalid", "Source-read metadata exceeds its limit")
    try:
        raw = base64.b64decode(header + "=" * (-len(header) % 4), altchars=b"-_", validate=True)
        scopes = _SCOPES.validate_json(raw)
    except (ValueError, binascii.Error, ValidationError) as error:
        raise DomainError("source_scope_invalid", "Invalid source-read metadata") from error
    if not scopes or len(scopes) > 32:
        raise DomainError("source_scope_invalid", "Source-read metadata requires 1 to 32 scopes")
    selected_subject = subject_id or scopes[0].subject_id
    if any(scope.subject_id != selected_subject for scope in scopes):
        raise DomainError(
            "source_scope_forbidden", "Source-read scope belongs to a different subject"
        )
    app: Application = request.app.state.application
    unrestricted = CallContext(
        caller_id="instance-admin", namespace_id=namespace_id, subject_id=selected_subject
    )
    clauses = [set(base_scope)] if base_scope is not None else []
    async with app.authority.read(unrestricted) as view:
        await require_subject_in(view, unrestricted)
        source_rows = (
            await view.connection.execute(
                select(observations.c.id, observations.c.active, observations.c.record).where(
                    observations.c.namespace_id == namespace_id,
                    observations.c.subject_id == selected_subject,
                )
            )
        ).all()
        active_ids = {str(row.id) for row in source_rows if row.active}
        sources = {
            source["id"]: source
            for source in (_SOURCE.validate_python(row.record) for row in source_rows)
        }
        material_observations: dict[str, set[str]] = {}
        for source in sources.values():
            material_observations.setdefault(source["material_id"], set()).add(source["id"])
        region_rows = (
            await view.connection.execute(
                select(regions.c.id, regions.c.material_id).where(
                    regions.c.namespace_id == namespace_id
                )
            )
        ).all()
        region_materials = {
            str(row.id): str(row.material_id)
            for row in region_rows
            if row.material_id in material_observations
        }

        async def cognition_selector(ref: CognitionSource) -> list[set[str]]:
            reference = ref.reference
            if isinstance(reference, str):
                resolution = await app.references.resolve_in(view, unrestricted, reference)
                if not isinstance(resolution.reference, CognitiveTarget):
                    raise DomainError(
                        "source_scope_invalid", "Cognition selector requires a cognitive address"
                    )
                reference = resolution.reference.reference
            await app.cognition.get_in(view, unrestricted, reference)
            return [{reference.exact()}]

        async def media_selector(ref: MediaSource) -> list[set[str]]:
            media = ref.reference
            if isinstance(media, str):
                resolution = await app.references.resolve_in(view, unrestricted, media)
                if not isinstance(resolution.reference, MediaTarget):
                    raise DomainError(
                        "source_scope_invalid", "Media selector requires a media address"
                    )
                media = resolution.reference.reference
            observation_id = ref.observation_id
            if isinstance(media, OriginalMedia):
                kind, identity = "material", media.material_id
            else:
                kind, identity = media.kind, media.id
                if (
                    observation_id is not None
                    and media.observation_id is not None
                    and observation_id != media.observation_id
                ):
                    raise DomainError(
                        "source_scope_invalid", "Media selector names conflicting observations"
                    )
                observation_id = observation_id or media.observation_id
            if kind not in {"material", "source_region"}:
                info = await app.derivations.source_in(
                    view,
                    unrestricted,
                    MediaReference.model_validate({"kind": kind, "id": identity}),
                )
                if observation_id is not None and observation_id not in info.roots:
                    raise DomainError(
                        "source_unavailable", "Selected observation did not produce this derivation"
                    )
                return (
                    [{info.reference.id}, {observation_id}]
                    if observation_id is not None
                    else [{info.reference.id}]
                )
            material_id = identity if kind == "material" else region_materials.get(identity)
            available = material_observations.get(material_id or "", set())
            if not available or (observation_id is not None and observation_id not in available):
                raise DomainError(
                    "source_unavailable", "Selected media has no authorized observation"
                )
            if kind == "material":
                return [{observation_id or identity}]
            return [{identity}, {observation_id}] if observation_id is not None else [{identity}]

        for scope in scopes:
            alternatives: list[list[set[str]]] = []
            for observation_id in scope.observation_ids:
                if observation_id not in sources:
                    raise DomainError("source_unavailable", "Selected observation is unavailable")
                alternatives.append([{observation_id}])
            pending = list(scope.source_refs)
            while pending:
                ref = pending.pop()
                if isinstance(ref, ObservationSource):
                    if ref.id not in sources:
                        raise DomainError(
                            "source_unavailable", "Selected observation is unavailable"
                        )
                    alternatives.append([{ref.id}])
                elif isinstance(ref, CognitionSource):
                    alternatives.append(await cognition_selector(ref))
                elif isinstance(ref, WorkContextSource):
                    value = await view.connection.scalar(
                        select(contexts.c.record).where(
                            contexts.c.id == ref.id,
                            contexts.c.namespace_id == namespace_id,
                            contexts.c.subject_id == selected_subject,
                        )
                    )
                    if value is None:
                        raise DomainError("context_not_found", "Selected context is unavailable")
                    targets: tuple[ContextTarget, ...]
                    if "work_context" in value:
                        targets = WorkContext.model_validate(value["work_context"]).content.anchors
                    else:
                        targets = tuple(
                            CognitiveTarget(reference=reference)
                            for reference in TypeAdapter(tuple[CognitiveRef, ...]).validate_python(
                                value["references"]
                            )
                        )
                    for target in targets:
                        if isinstance(target, CognitiveTarget):
                            alternatives.append(
                                await cognition_selector(
                                    CognitionSource(reference=target.reference)
                                )
                            )
                        else:
                            alternatives.append(
                                await media_selector(MediaSource(reference=target.reference))
                            )
                else:
                    alternatives.append(await media_selector(ref))
            for source in sources.values():
                if (
                    source["id"] in active_ids
                    and source["origin"].get("conversation_ref") in scope.origin_conversation_refs
                ):
                    alternatives.append([{source["id"]}])
            clauses.extend(union_selectors(alternatives))
    return resolved_scope(clauses)
