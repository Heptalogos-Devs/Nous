"""Scope-bound keyset pages over authority links and live DBOS-owned state."""

import base64
import hashlib
from dataclasses import dataclass

import sqlalchemy as sa
from dbos import DBOSClient, WorkflowStatus
from nous.platform.call_context import CallContext
from nous_sdk.contracts.execution import ExecutionJob
from pydantic import BaseModel, ConfigDict, Field

from nous.platform.persistence import Authority
from nous.platform.persistence.authority import canonical_json
from nous.platform.persistence.schema import job_links

from .results import job_results, retained_status
from .scope import JobPurpose, require_job_purpose

_STATES = frozenset(
    {
        "PENDING",
        "SUCCESS",
        "ERROR",
        "MAX_RECOVERY_ATTEMPTS_EXCEEDED",
        "CANCELLED",
        "ENQUEUED",
        "DELAYED",
    }
)


@dataclass(frozen=True)
class JobPage:
    items: tuple[WorkflowStatus, ...]
    next_cursor: str | None = None


class _Cursor(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)
    binding: str = Field(min_length=64, max_length=64)
    snapshot: int = Field(ge=0)
    sequence: int = Field(gt=0)
    workflow_id: str = Field(min_length=1, max_length=256)

    def encode(self) -> str:
        return base64.urlsafe_b64encode(self.model_dump_json().encode()).decode().rstrip("=")


async def query_jobs(
    authority: Authority,
    client: DBOSClient,
    ctx: CallContext,
    *,
    scope: str | None = None,
    kind: str | None = None,
    state: str | None = None,
    limit: int = 50,
    cursor: str | None = None,
    purpose: JobPurpose = "actor",
    workflow_ids: tuple[str, ...] | None = None,
) -> JobPage:
    """Newest admission first; state stays live while new admissions cannot move a page.

    DBOS's public query performs state/name filtering in SQL. Authority links are
    scanned in bounded batches until the page fills, so sparse matches are never
    lost behind a limit applied to unfiltered jobs. No scheduling state is copied.
    """
    require_job_purpose(ctx, purpose)
    if not 1 <= limit <= 1000:
        raise ValueError("Job page limit must be between 1 and 1000")
    if state is not None and state not in _STATES:
        raise ValueError("Unknown DBOS job state")
    if kind is not None and not kind:
        raise ValueError("Job kind must be a workflow name")
    binding = hashlib.sha256(
        canonical_json(
            (
                ctx.caller_id,
                ctx.namespace_id,
                ctx.namespace_id,
                scope,
                kind,
                state,
                purpose,
                sorted(workflow_ids) if workflow_ids is not None else None,
            )
        ).encode()
    ).hexdigest()
    position: _Cursor | None = None
    if cursor is not None:
        try:
            if len(cursor) > 2048:
                raise ValueError("Cursor is too long")
            position = _Cursor.model_validate_json(
                base64.b64decode(cursor + "=" * (-len(cursor) % 4), altchars=b"-_", validate=True)
            )
            if position.binding != binding:
                raise ValueError("Cursor belongs to a different query")
        except ValueError as error:
            raise ValueError("Invalid job cursor for this scope and query") from error

    query = sa.select(job_links.c.workflow_id, job_links.c.commit_seq).where(
        job_links.c.namespace_id == ctx.namespace_id,
    )
    if purpose == "actor":
        query = query.where(job_links.c.caller_id == ctx.caller_id)
    if ctx.namespace_id is not None:
        query = query.where(job_links.c.namespace_id == ctx.namespace_id)
    if scope is not None:
        query = query.where(job_links.c.scope == scope)
    if kind is not None:
        query = query.where(job_links.c.workflow_name == kind)
    if workflow_ids is not None:
        query = query.where(job_links.c.workflow_id.in_(workflow_ids))
    collected: list[tuple[WorkflowStatus, int]] = []
    batch_size = max(128, min(limit + 1, 512))
    async with authority.read(ctx) as view:
        snapshot = position.snapshot if position is not None else view.commit_seq
        if snapshot > view.commit_seq:
            raise ValueError("Job cursor refers to an unavailable snapshot")
        query = query.where(job_links.c.commit_seq <= snapshot)
        while True:
            page = query
            if position is not None:
                page = page.where(
                    sa.or_(
                        job_links.c.commit_seq < position.sequence,
                        sa.and_(
                            job_links.c.commit_seq == position.sequence,
                            job_links.c.workflow_id < position.workflow_id,
                        ),
                    )
                )
            rows = (
                await view.connection.execute(
                    page.order_by(
                        job_links.c.commit_seq.desc(), job_links.c.workflow_id.desc()
                    ).limit(batch_size)
                )
            ).all()
            if not rows:
                break
            ids = [str(row.workflow_id) for row in rows]
            matches = await client.list_workflows_async(
                workflow_ids=ids,
                name=kind,
                status=state,
                limit=len(ids),
                load_input=False,
                load_output=False,
            )
            by_id = {item.workflow_id: item for item in matches}
            archived = (
                await view.connection.execute(
                    sa.select(job_results.c.record).where(
                        job_results.c.workflow_id.in_(ids),
                        job_results.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).scalars()
            for raw in archived:
                saved = ExecutionJob.model_validate(raw)
                if (kind is None or saved.workflow_name == kind) and (
                    state is None or saved.state == state
                ):
                    by_id[saved.workflow_id] = retained_status(saved)
            for row in rows:
                item = by_id.get(str(row.workflow_id))
                if item is not None:
                    collected.append((item, int(row.commit_seq)))
                    if len(collected) > limit:
                        last, sequence = collected[-2]
                        return JobPage(
                            tuple(item for item, _ in collected[:-1]),
                            _Cursor(
                                binding=binding,
                                snapshot=snapshot,
                                sequence=sequence,
                                workflow_id=last.workflow_id,
                            ).encode(),
                        )
            if len(rows) < batch_size:
                break
            position = _Cursor(
                binding=binding,
                snapshot=snapshot,
                sequence=int(rows[-1].commit_seq),
                workflow_id=str(rows[-1].workflow_id),
            )
    return JobPage(tuple(item for item, _ in collected))
