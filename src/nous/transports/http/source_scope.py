"""Resolve public source selectors without broadening precise selections."""

import base64
import binascii

from fastapi import Request
from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
)
from nous_wave.source_scope import (
    CognitionSource,
    ObservationSource,
    OriginalMedia,
    SourceReadScope,
    WorkContextSource,
)
from pydantic import TypeAdapter, ValidationError
from sqlalchemy import select

from nous.application import Application
from nous.context.service import contexts
from nous.identity.service import require_subject_in
from nous.material.service import SourceRecord, observations, regions

_SCOPES = TypeAdapter(tuple[SourceReadScope, ...])
_SOURCE = TypeAdapter(SourceRecord)


async def resolve_scope(
    request: Request,
    namespace_id: str,
    subject_id: str | None,
    base_scope: tuple[str, ...] | None = None,
) -> tuple[str, ...] | None:
    header = request.headers.get("x-nous-source-read")
    if header is None:
        return base_scope
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
        exact_records: dict[str, CognitiveRecord] = {}
        selections: list[set[str]] = [set(base_scope)] if base_scope is not None else []
        broad_object_ids: set[str] = set()
        if base_scope is not None:
            from nous.cognition.service import revisions

            for token in base_scope:
                if token in sources or token in material_observations or token in region_materials:
                    continue
                query = select(revisions.c.record).where(
                    revisions.c.namespace_id == namespace_id,
                    revisions.c.subject_id == selected_subject,
                )
                if ":" in token and "@" in token:
                    kind, versioned = token.split(":", 1)
                    object_id, version = versioned.rsplit("@", 1)
                    if not version.isdigit():
                        continue
                    query = query.where(
                        revisions.c.id == object_id,
                        revisions.c.revision == int(version),
                        revisions.c.kind == kind,
                    )
                else:
                    broad_object_ids.add(token)
                    query = query.where(revisions.c.id == token, revisions.c.head.is_(True))
                raw_record = (await view.connection.execute(query)).scalar_one_or_none()
                if raw_record is not None:
                    exact_records[token] = CognitiveRecord.model_validate(raw_record)

        for scope in scopes:
            selected: set[str] = set()
            for observation_id in scope.observation_ids:
                if observation_id not in sources:
                    raise DomainError("source_unavailable", "Selected observation is unavailable")
                selected.add(observation_id)
            for ref in scope.source_refs:
                if isinstance(ref, ObservationSource):
                    if ref.id not in sources:
                        raise DomainError(
                            "source_unavailable", "Selected observation is unavailable"
                        )
                    selected.add(ref.id)
                elif isinstance(ref, CognitionSource):
                    record = await app.cognition.get_in(view, unrestricted, ref.reference)
                    exact_records[ref.reference.exact()] = record
                    selected.add(ref.reference.exact())
                elif isinstance(ref, WorkContextSource):
                    value = (
                        await view.connection.execute(
                            select(contexts.c.record).where(
                                contexts.c.id == ref.id,
                                contexts.c.namespace_id == namespace_id,
                                contexts.c.subject_id == selected_subject,
                            )
                        )
                    ).scalar_one_or_none()
                    if value is None:
                        raise DomainError("context_not_found", "Selected context is unavailable")
                    references = TypeAdapter(tuple[CognitiveRef, ...]).validate_python(
                        value["references"]
                    )
                    for reference in references:
                        record = await app.cognition.get_in(view, unrestricted, reference)
                        exact_records[reference.exact()] = record
                        selected.add(reference.exact())
                else:
                    media = ref.reference
                    if isinstance(media, OriginalMedia):
                        kind, identity = "material", media.material_id
                    else:
                        kind, identity = media.kind, media.id
                    material_id = identity if kind == "material" else region_materials.get(identity)
                    if kind not in {"material", "source_region"}:
                        raise DomainError(
                            "source_unavailable", "Selected media derivation is unavailable"
                        )
                    available = material_observations.get(material_id or "", set())
                    if not available or (
                        ref.observation_id is not None and ref.observation_id not in available
                    ):
                        raise DomainError(
                            "source_unavailable",
                            "Selected media has no authorized live observation",
                        )
                    selected.add(
                        ref.observation_id
                        if kind == "material" and ref.observation_id is not None
                        else identity
                    )
            for source in sources.values():
                if (
                    source["id"] in active_ids
                    and source["origin"].get("conversation_ref") in scope.origin_conversation_refs
                ):
                    selected.add(source["id"])
            selections.append(selected)

        async def admitted(token: str, selection: set[str]) -> bool:
            if token in selection:
                return True
            if token in sources:
                return sources[token]["material_id"] in selection
            if token in region_materials:
                material = region_materials[token]
                return material in selection or bool(
                    material_observations[material].intersection(selection)
                )
            if token in broad_object_ids:
                return False
            if token in exact_records:
                narrowed = unrestricted.model_copy(update={"allowed_source_ids": tuple(selection)})
                return await app.cognition.visible_in(view, narrowed, exact_records[token])
            return False

        candidates = {token for selection in selections for token in selection}
        resolved: list[str] = []
        for token in sorted(candidates):
            if all([await admitted(token, selection) for selection in selections]):
                resolved.append(token)
        return tuple(resolved)
