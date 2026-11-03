"""One source-withdrawal projection for current and historical proposal content."""

from nous_sdk.contracts.maintenance import MaintenanceRun


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
