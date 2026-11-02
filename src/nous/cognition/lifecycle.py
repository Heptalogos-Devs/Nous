"""Memory retention and transitions adopted from the existing Nous owner.

The original implementation is crates/nous-app/src/cognition/maintenance.rs.
Protection, source lifetime, deletion and activation retain separate meanings.
"""

from nous_sdk.contracts import (
    CognitiveBody,
    CognitiveRecord,
    ConceptBody,
    EntityBody,
    EpisodeBody,
    MemoryChange,
    RetentionChange,
    RetentionSnapshot,
    RetentionState,
)
from nous.platform.errors import DomainError
from nous_sdk.contracts.fact_body import FactClaimBody

HOUR_MS = 3600000
HALF_LIFE_HOURS = 24.0
FREEZE_THRESHOLD = 0.1
REVIVE_THRESHOLD = 0.15


def active_body(body: CognitiveBody | None, lifecycle: RetentionState | None = None) -> bool:
    """One activation rule also serves planned growth before a revision exists."""
    if body is None:
        return False
    if lifecycle is not None and lifecycle.activation != "active":
        return False
    if isinstance(body, (ConceptBody, EpisodeBody, EntityBody)):
        return body.state == "active"
    if isinstance(body, FactClaimBody):
        return body.status == "active"
    return True


def is_active(record: CognitiveRecord) -> bool:
    """Current domain activation; source access and deletion remain separate decisions."""
    return not record.purged and active_body(record.body, record.lifecycle)


def record_state(record: CognitiveRecord) -> str:
    if record.purged:
        return "purged"
    if record.deleted:
        return "deleted"
    if record.lifecycle is not None and record.lifecycle.activation != "active":
        return record.lifecycle.activation
    body = record.body
    if isinstance(body, FactClaimBody):
        return body.status
    if isinstance(body, (ConceptBody, EpisodeBody, EntityBody)):
        return body.state
    return "active"


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


def retention(state: RetentionState, now: int) -> float:
    return state.strength * 2 ** (-max(0, now - state.anchor_unix_ms) / (HALF_LIFE_HOURS * HOUR_MS))


def state_for(record: CognitiveRecord) -> RetentionState:
    if (
        record.reference.kind not in {"memory", "association"}
        or record.purged
        or record.body is None
    ):
        raise DomainError(
            "retention_unavailable", "Retention requires a nonpurged Memory or Association"
        )
    return record.lifecycle or RetentionState(anchor_unix_ms=record.times.recorded.coordinate)


def snapshot(record: CognitiveRecord, now: int) -> RetentionSnapshot:
    state = state_for(record)
    reason = {
        "active": "",
        "frozen": "manual_freeze",
        "forgotten": "manual_forget",
        "weakened": "weaken",
        "decayed": "decay",
    }[state.activation]
    return RetentionSnapshot(
        reference=record.reference,
        strength=state.strength,
        anchor_unix_ms=state.anchor_unix_ms,
        inactive_since_unix_ms=state.inactive_since_unix_ms,
        inactive_reason=reason,
        inactive=state.activation != "active",
        permanent=state.permanent,
        pinned=state.pinned,
        protected_until_unix_ms=state.protected_until_unix_ms,
        score=retention(state, now),
    )


def transition(
    state: RetentionState, command: MemoryChange | RetentionChange, now: int
) -> RetentionState:
    now = max(
        now,
        state.anchor_unix_ms,
        state.inactive_since_unix_ms or now,
        state.last_reinforced_unix_ms or now,
    )
    current = retention(state, now)
    result = state.model_dump()
    action = command.action
    if action in {"trash", "purge"}:
        return state
    if action == "remember_forever":
        return state.model_copy(
            update={
                "permanent": True,
                "strength": 1.0,
                "activation": "active",
                "anchor_unix_ms": now,
                "inactive_since_unix_ms": None,
                "last_reinforced_unix_ms": now,
            }
        )
    if action == "allow_decay":
        return state.model_copy(update={"permanent": False})
    if action in {"pin", "unpin"}:
        return state.model_copy(update={"pinned": action == "pin"})
    if action == "protect":
        result.update(
            pinned=command.hours is None or command.hours <= 0,
            protected_until_unix_ms=None
            if command.hours is None or command.hours <= 0
            else now + int(command.hours * HOUR_MS),
        )
        return RetentionState.model_validate(result)
    if action == "unprotect":
        return state.model_copy(update={"pinned": False, "protected_until_unix_ms": None})
    if action == "decay":
        if (
            state.activation == "active"
            and (not state.protected(now))
            and (current <= FREEZE_THRESHOLD)
        ):
            result.update(activation="decayed", inactive_since_unix_ms=now)
        return RetentionState.model_validate(result)
    if action == "freeze":
        result.update(strength=current, activation="frozen")
    elif action == "forget":
        result.update(
            strength=0,
            activation="forgotten",
            pinned=False,
            permanent=False,
            protected_until_unix_ms=None,
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
    return RetentionState.model_validate(result)
