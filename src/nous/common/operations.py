"""Root operation diagnostics and structured exception context."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from contextvars import ContextVar
from dataclasses import dataclass, field
from typing import Literal

from pydantic import BaseModel, JsonValue, TypeAdapter
from pydantic_core import to_jsonable_python


@dataclass(frozen=True)
class OperationIssue:
    """A classified unfinished stage, shared by the response and root diagnostics."""

    stage: str
    code: str
    message: str
    details: dict[str, JsonValue] = field(default_factory=dict)
    retryable: bool = False
    affected_refs: tuple[str, ...] = ()
    completed_refs: tuple[dict[str, JsonValue], ...] = ()

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            "stage": self.stage,
            "code": self.code,
            "message": self.message,
            "details": dict(self.details),
            "retryable": self.retryable,
            "affected_refs": list[JsonValue](self.affected_refs),
            "completed_refs": list[JsonValue](self.completed_refs),
        }


@dataclass
class OperationDiagnostics:
    operation_id: str
    name: str
    attempts: list[dict[str, JsonValue]] = field(default_factory=list)
    completed_refs: list[dict[str, JsonValue]] = field(default_factory=list)
    outcome: Literal["success", "partial"] = "success"
    issues: list[OperationIssue] = field(default_factory=list)


@dataclass(frozen=True)
class OperationFrame:
    root: OperationDiagnostics
    stage: str


current_operation: ContextVar[OperationFrame | None] = ContextVar(
    "nous_operation_frame", default=None
)


def reference_value(ref: BaseModel | Mapping[str, JsonValue]) -> dict[str, JsonValue]:
    """Encode one formal reference for diagnostics and transport."""
    return TypeAdapter(dict[str, JsonValue]).validate_python(to_jsonable_python(ref))


def record_completed(
    refs: BaseModel | Mapping[str, JsonValue] | Sequence[BaseModel | Mapping[str, JsonValue]],
) -> None:
    """Retain adopted outputs on their root operation even when later stages are cancelled."""
    frame = current_operation.get()
    if frame is None:
        return
    for ref in (refs,) if isinstance(refs, (BaseModel, Mapping)) else refs:
        value = reference_value(ref)
        if value not in frame.root.completed_refs:
            frame.root.completed_refs.append(value)


def completed_references() -> tuple[dict[str, JsonValue], ...]:
    frame = current_operation.get()
    return tuple(frame.root.completed_refs) if frame else ()


def report_outcome(
    status: Literal["success", "partial"],
    *,
    issues: Sequence[OperationIssue] = (),
    completed_refs: Sequence[BaseModel | Mapping[str, JsonValue]] = (),
) -> None:
    """Report a complete use case explicitly; recovered attempts do not call this."""
    frame = current_operation.get()
    if frame is None:
        return
    if status == "partial":
        frame.root.outcome = "partial"
    for issue in issues:
        if issue not in frame.root.issues:
            frame.root.issues.append(issue)
        record_completed(issue.completed_refs)
    record_completed(completed_refs)


def exception_context(error: BaseException) -> dict[str, JsonValue]:
    value = getattr(error, "nous_context", None)
    return (
        TypeAdapter(dict[str, JsonValue]).validate_python(value) if isinstance(value, dict) else {}
    )


def add_error_context(error: BaseException, values: Mapping[str, object]) -> None:
    context = {
        **exception_context(error),
        **TypeAdapter(dict[str, JsonValue]).validate_python(dict(values)),
    }
    error.__dict__["nous_context"] = context
    from .errors import NousError

    if isinstance(error, NousError):
        error.details.update(context)


def attach_context(error: BaseException, frame: OperationFrame, trace_id: str | None) -> None:
    """Preserve cancellation/timeout types while attaching one shared diagnostic value."""
    from .errors import NousError

    previous = exception_context(error)
    details: dict[str, JsonValue] = {
        **previous,
        "operation_id": frame.root.operation_id,
        "stage": previous.get("stage", frame.stage),
        "attempts": list[JsonValue](frame.root.attempts),
        "completed_refs": list[JsonValue](frame.root.completed_refs),
        "issues": [issue.to_dict() for issue in frame.root.issues],
    }
    if trace_id is not None:
        details["trace_id"] = trace_id
    error.__dict__["nous_context"] = details
    if isinstance(error, NousError):
        error.operation_id = error.operation_id or frame.root.operation_id
        error.trace_id = error.trace_id or trace_id
        error.operation = error.operation or frame.root.name
        error.details.update(
            {
                key: value
                for key, value in details.items()
                if key not in {"operation_id", "trace_id"}
            }
        )
