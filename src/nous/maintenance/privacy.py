"""One source-withdrawal projection for current and historical proposal content."""

from nous_sdk.contracts.maintenance import MaintenanceRun
from pydantic import JsonValue


def redact(record: MaintenanceRun) -> MaintenanceRun:
    return record.model_copy(
        update={
            "sources_available": False,
            "status": "unavailable",
            "draft": None,
            "proposal": None,
            "edits": (),
            "groups": (),
            "questions": (),
            "catalog": None,
            "command": record.command.model_copy(update={"instruction": ""}),
            "review": None
            if record.review is None
            else record.review.model_copy(update={"reason": "", "adopted": ()}),
            "review_history": tuple(
                review.model_copy(update={"reason": "", "adopted": ()})
                for review in record.review_history
            ),
        }
    )


def receipt_contains_content(value: JsonValue, run_id: str) -> bool:
    if isinstance(value, dict):
        if (value.get("id") == run_id or value.get("run_id") == run_id) and any(
            key in value
            for key in (
                "proposal",
                "edits",
                "draft",
                "review",
                "review_history",
                "groups",
                "questions",
                "instruction",
            )
        ):
            return True
        return any(receipt_contains_content(child, run_id) for child in value.values())
    if isinstance(value, list):
        return any(receipt_contains_content(child, run_id) for child in value)
    return False
