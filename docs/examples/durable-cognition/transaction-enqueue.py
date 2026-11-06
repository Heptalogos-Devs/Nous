"""Register maintenance within the same transaction as an application's commit."""

from pydantic import JsonValue

from nous.jobs import JobRef
from nous.jobs.operations import JobOperations
from nous.storage.database import WriteTx


async def enqueue_refresh(
    tx: WriteTx, jobs: JobOperations, workflow_id: str, payload: dict[str, JsonValue]
) -> JobRef:
    return await jobs.enqueue_in(tx, kind="refresh", workflow_id=workflow_id, payload=payload)
