"""Memory retention and transitions adopted from the existing Nous owner.

The original implementation is crates/nous-app/src/cognition/maintenance.rs.
Protection, source lifetime, deletion and activation retain separate meanings.
"""

from nous_wave.contracts import MemoryChange, MemoryLifecycle

HOUR_MS = 3600000
FREEZE_THRESHOLD = 0.1
REVIVE_THRESHOLD = 0.15


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
