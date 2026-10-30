"""Memory retention and transitions adopted from the existing Nous owner.

The original implementation is crates/nous-app/src/cognition/maintenance.rs.
Protection, source lifetime, deletion and activation retain separate meanings.
"""

from nous_sdk.contracts import (
    CognitiveBody,
    CognitiveRecord,
    ConceptBody,
    EpisodeBody,
    MemoryChange,
    MemoryLifecycle,
)
from nous_sdk.contracts.fact_body import FactClaimBody

HOUR_MS = 3600000
FREEZE_THRESHOLD = 0.1
REVIVE_THRESHOLD = 0.15


def active_body(body: CognitiveBody | None, lifecycle: MemoryLifecycle | None = None) -> bool:
    """One activation rule also serves planned growth before a revision exists."""
    if body is None:
        return False
    if lifecycle is not None and lifecycle.activation != "active":
        return False
    if isinstance(body, (ConceptBody, EpisodeBody)):
        return body.state == "active"
    if isinstance(body, FactClaimBody):
        return body.status == "active"
    return True


def is_active(record: CognitiveRecord) -> bool:
    """Current domain activation; source access and deletion remain separate decisions."""
    return not record.purged and active_body(record.body, record.lifecycle)


def selectable(
    record: CognitiveRecord, *, include_deleted: bool = False, include_inactive: bool = False
) -> bool:
    """Automatic candidates share lifecycle rules without changing exact/history reads."""
    return (
        not record.purged
        and record.body is not None
        and (include_deleted or not record.deleted)
        and (include_inactive or is_active(record))
    )


def retention(state: MemoryLifecycle, now: int) -> float:
    return state.strength * 2 ** (-max(0, now - state.anchor_unix_ms) / (24 * HOUR_MS))


def transition(state: MemoryLifecycle, command: MemoryChange, now: int) -> MemoryLifecycle:
    now = max(
        now,
        state.anchor_unix_ms,
        state.inactive_since_unix_ms or now,
        state.last_reinforced_unix_ms or now,
    )
    current = retention(state, now)
    result = state.model_dump()
    action = command.action
    if action == "protect":
        result.update(
            pinned=command.hours is None or command.hours <= 0,
            protected_until_unix_ms=None
            if command.hours is None or command.hours <= 0
            else now + int(command.hours * HOUR_MS),
        )
        return MemoryLifecycle.model_validate(result)
    if action == "unprotect":
        return state.model_copy(update={"pinned": False, "protected_until_unix_ms": None})
    if action == "decay":
        if (
            state.activation == "active"
            and not state.protected(now)
            and current <= FREEZE_THRESHOLD
        ):
            result.update(activation="decayed", inactive_since_unix_ms=now)
        return MemoryLifecycle.model_validate(result)
    if action == "freeze":
        result.update(strength=current, activation="frozen")
    elif action == "forget":
        result.update(
            strength=0, activation="forgotten", pinned=False, protected_until_unix_ms=None
        )
    elif action == "reinforce":
        strength = min(1, current + min(1, 0.5 * command.strength) * (1 - current))
        result.update(
            strength=strength,
            last_reinforced_unix_ms=now,
            protected_until_unix_ms=max(
                state.protected_until_unix_ms or now,
                now + int(max(1, 24 * command.strength) * HOUR_MS),
            ),
        )
        if strength >= REVIVE_THRESHOLD:
            result["activation"] = "active"
    elif action == "weaken":
        strength = max(0, current * (1 - min(1, 0.5 * command.strength)))
        result["strength"] = strength
        if strength <= FREEZE_THRESHOLD:
            result["activation"] = "weakened"
        elif strength >= REVIVE_THRESHOLD:
            result["activation"] = "active"
    elif action == "restore":
        result.update(
            activation="active",
            strength=max(REVIVE_THRESHOLD, current),
            inactive_since_unix_ms=None,
        )
    result["anchor_unix_ms"] = now
    result["inactive_since_unix_ms"] = (
        None if result["activation"] == "active" else state.inactive_since_unix_ms or now
    )
    return MemoryLifecycle.model_validate(result)
