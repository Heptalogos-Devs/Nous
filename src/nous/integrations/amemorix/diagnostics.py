"""Management diagnostics compose the owners' observed resource and vector reports."""

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext
from pydantic import JsonValue

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


async def diagnostics(service: AMemorixService, ctx: CallContext) -> dict[str, JsonValue]:
    retrieval = await service.app.retrieval.status(ctx)
    imports = await service.app.imports.vector_status(ctx)
    models = await service.app.models.status(ctx)
    policy = await service.policy(ctx)
    configuration = await service.app.configuration.effective(ctx, "models")
    dimensions = {space.dimensions for space in retrieval.spaces}
    spaces = [
        {
            "space_id": value.space,
            "space": value,
            "embedding_fingerprint": {
                "model": value.model,
                "dimension": value.dimensions,
                "hash": value.space,
            },
            "state": "saved",
            "can_delete": False,
            "vector_count": value.indexed_references,
            "size_bytes": None,
            "missing_references": value.missing_references,
            "unknown": {
                "size_bytes": "Immutable spaces share database storage.",
                "active": "The configured provider fingerprint has not been resolved.",
            },
        }
        for value in retrieval.spaces
    ]
    unknown = {
        "embedding_fingerprint": "Configured model aliases have no resolved immutable fingerprint.",
        "provider_ready": "Provider availability has not been probed.",
        "paragraph_vector_backfill": "No configured-space paragraph task inventory was requested.",
        "auto_save": "Writes commit through the authority; no separate auto-save state exists.",
        "relation_vectors_enabled": "No independent relation-vector policy exists.",
        "vector_pools": "No independent vector-pool lifecycle exists.",
        "data_dir": "Subject diagnostics do not expose the instance directory.",
    }
    return json_object(
        {
            "success": True,
            "config": configuration,
            "integration": policy,
            "models": models,
            "retrieval": retrieval,
            "imports": imports,
            "items": spaces,
            "spaces": spaces,
            "vector_spaces": spaces,
            "watermarks": {
                "retrieval": retrieval.authority_commit_seq,
                "imports": imports.authority_commit_seq,
            },
            "state": "ready" if spaces else "empty",
            "data_dir": None,
            "embedding_dimension": None,
            "stored_vector_dimension": next(iter(dimensions)) if len(dimensions) == 1 else None,
            "embedding_fingerprint": None,
            "stored_embedding_fingerprint": None,
            "embedding_fingerprint_status": "unknown",
            "vector_rebuild_required": None,
            "vector_rebuild_message": "",
            "auto_save": None,
            "relation_vectors_enabled": None,
            "vector_pools": None,
            "vector_pools_ready": None,
            "vector_pools_effective_mode": None,
            "memory_enabled": policy.enabled,
            "runtime_ready": True,
            "retrieval_ready": policy.enabled,
            "available_channels": ["metadata", "sparse"] + (["vector_read"] if spaces else []),
            "unavailable_channels": [],
            "embedding_degraded": None,
            "embedding_degraded_reason": None,
            "embedding_degraded_since": None,
            "embedding_last_check": None,
            "paragraph_vector_backfill_pending": None,
            "paragraph_vector_backfill_running": None,
            "paragraph_vector_backfill_failed": None,
            "paragraph_vector_backfill_done": None,
            "unknown": unknown,
            "available_actions": ["get_config", "list_vector_spaces", "set_config"],
        }
    )
