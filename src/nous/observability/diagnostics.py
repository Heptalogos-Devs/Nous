"""Read retained operation events from the existing rotating JSONL log."""

from pathlib import Path

from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError

_EVENT = TypeAdapter(dict[str, JsonValue])


def operation_logs(directory: Path, prefix: str) -> dict[str, object]:
    if not prefix:
        raise NousError("diagnostics.operation_required", "Provide an operation ID or prefix")
    events: list[dict[str, JsonValue]] = []
    matches: set[str] = set()
    for file in directory.glob("nous.jsonl*"):
        if not file.is_file():
            continue
        with file.open("r", encoding="utf-8") as stream:
            for line in stream:
                try:
                    event = _EVENT.validate_json(line)
                except ValueError:
                    continue
                identifier = event.get("operation_id")
                if isinstance(identifier, str) and identifier.startswith(prefix):
                    matches.add(identifier)
                    events.append(event)
    if not matches:
        raise NousError(
            "diagnostics.operation_not_found", "No retained log events match this operation"
        )
    if len(matches) != 1:
        raise NousError(
            "diagnostics.operation_ambiguous",
            "Operation prefix matches several operations",
            details={"candidates": sorted(matches)},
        )
    events.sort(key=lambda item: str(item.get("timestamp", "")))
    attempts = [event for event in events if event.get("event") == "request.attempt"]
    failure = next(
        (event for event in reversed(events) if event.get("event") == "operation.failed"), None
    )
    return {
        "operation_id": next(iter(matches)),
        "operation": events[0].get("operation"),
        "result": failure.get("result") if failure else events[-1].get("result", "unknown"),
        "attempts": attempts,
        "events": events,
        "error": failure,
        "retention": "available_rotating_logs",
    }
