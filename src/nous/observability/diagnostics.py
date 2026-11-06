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
    terminal = next(
        (event for event in reversed(events) if event.get("event") == "operation.terminal"), None
    )
    return {
        "operation_id": next(iter(matches)),
        "operation": events[0].get("operation"),
        "result": terminal.get("result") if terminal else "incomplete",
        "attempts": attempts,
        "events": events,
        "error": terminal if terminal and terminal.get("result") != "success" else None,
        "issues": terminal.get("issues", []) if terminal else [],
        "completed_result_refs": terminal.get("completed_result_refs", []) if terminal else [],
        "retention": "available_rotating_logs",
    }


def recent_problems(directory: Path, limit: int = 5) -> list[dict[str, JsonValue]]:
    """Read unfinished root outcomes; recovered attempts remain in operation detail."""
    if limit <= 0:
        return []
    problems: dict[str, dict[str, JsonValue]] = {}
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
                if (
                    event.get("event") == "operation.terminal"
                    and isinstance(identifier, str)
                    and event.get("result") in {"error", "partial", "timeout", "cancelled"}
                ):
                    previous = problems.get(identifier)
                    if previous is None or str(event.get("timestamp", "")) > str(
                        previous.get("timestamp", "")
                    ):
                        problems[identifier] = {
                            key: event.get(key)
                            for key in (
                                "timestamp",
                                "operation_id",
                                "operation",
                                "stage",
                                "result",
                                "error_code",
                                "error_message",
                                "error_type",
                                "details",
                                "issues",
                                "completed_result_refs",
                            )
                        }
    return sorted(
        problems.values(), key=lambda event: str(event.get("timestamp", "")), reverse=True
    )[:limit]


def recent_content_preparations(directory: Path, limit: int = 3) -> list[dict[str, JsonValue]]:
    """Read completed build summaries without opening an index or preparing content."""
    if limit <= 0:
        return []
    builds: list[dict[str, JsonValue]] = []
    for file in directory.glob("nous.jsonl*"):
        if not file.is_file():
            continue
        with file.open("r", encoding="utf-8") as stream:
            for line in stream:
                try:
                    event = _EVENT.validate_json(line)
                except ValueError:
                    continue
                if event.get("event") == "wave.build.completed":
                    builds.append(
                        {
                            key: event.get(key)
                            for key in (
                                "timestamp",
                                "operation_id",
                                "snapshot",
                                "preparation",
                                "stages_ms",
                            )
                        }
                    )
    return sorted(builds, key=lambda event: str(event.get("timestamp", "")), reverse=True)[:limit]
