"""Bounded physical retries only after an explicit provider rate-limit rejection."""

import asyncio
from collections.abc import Awaitable, Callable
from hashlib import sha256

from nous_sdk.contracts import CallContext
from nous_sdk.contracts.model_routing import ModelRoutingPolicy

from nous.platform.errors import Unavailable

from .ledger import CallLedger
from .outcomes import may_continue


def manual_identity(base: str, authorization: str) -> str:
    return base + ":manual:" + sha256(authorization.encode()).hexdigest()


async def needs_provider_request(
    ctx: CallContext,
    role: str,
    identities: tuple[str, ...],
    policy: ModelRoutingPolicy,
    ledger: CallLedger,
    authorization: str | None = None,
) -> bool:
    for base in identities:
        latest = None
        latest_id = base
        latest_index = 0
        for index in range(policy.rate_limit_retries + 1):
            identity = base if index == 0 else f"{base}:rate-limit:{index}"
            actual = await ledger.attempt(ctx, role, identity)
            if actual is None:
                break
            latest, latest_id, latest_index = actual, identity, index
        if latest is None:
            return True
        manual = (
            await ledger.attempt(ctx, role, manual_identity(base, authorization))
            if authorization
            else None
        )
        if manual is not None:
            latest, latest_id = manual, manual_identity(base, authorization or "")
        elif (
            authorization
            and latest.status != "succeeded"
            and latest.outcome is not None
            and latest.outcome.kind == "rejected"
            and may_continue(policy, latest.outcome)
        ):
            return True
        if latest.status == "succeeded":
            continue
        if latest.outcome is not None and latest.outcome.kind == "not_sent" and policy.not_sent:
            return True
        if not may_continue(policy, latest.outcome):
            await ledger.recall(ctx, role, latest_id)
        if manual is not None:
            return False
        return bool(
            latest.outcome is not None
            and latest.outcome.kind == "rejected"
            and latest.outcome.http_status == 429
            and latest_index < policy.rate_limit_retries
        )
    return False


async def rate_limited[T](
    ctx: CallContext,
    role: str,
    invocation_id: str | None,
    policy: ModelRoutingPolicy,
    ledger: CallLedger,
    request: Callable[[str | None], Awaitable[T]],
    *,
    authorization: str | None = None,
) -> T:
    if invocation_id is not None and authorization is not None:
        manual = manual_identity(invocation_id, authorization)
        prior = await ledger.attempt(ctx, role, manual)
        if prior is not None:
            return await request(manual)
        latest = None
        latest_id = invocation_id
        for index in range(policy.rate_limit_retries + 1):
            identity = invocation_id if index == 0 else f"{invocation_id}:rate-limit:{index}"
            found = await ledger.attempt(ctx, role, identity)
            if found is None:
                break
            latest, latest_id = found, identity
        if latest is not None:
            if (
                latest.status == "succeeded"
                or latest.outcome is not None
                and latest.outcome.kind == "not_sent"
                and policy.not_sent
            ):
                return await request(latest_id)
            if (
                latest.outcome is not None
                and latest.outcome.kind == "rejected"
                and may_continue(policy, latest.outcome)
            ):
                if latest.outcome.http_status == 429:
                    delay = latest.outcome.retry_after_seconds
                    delay = policy.rate_limit_wait_seconds if delay is None else delay
                    if delay > policy.max_retry_after_seconds:
                        await ledger.recall(ctx, role, latest_id)
                    await asyncio.sleep(delay)
                return await request(manual)
            await ledger.recall(ctx, role, latest_id)
    if invocation_id is None or not policy.rate_limit_retries:
        return await request(invocation_id)
    for index in range(policy.rate_limit_retries + 1):
        identity = invocation_id if index == 0 else f"{invocation_id}:rate-limit:{index}"
        prior = await ledger.attempt(ctx, role, identity)
        if (
            prior is None
            or prior.status == "succeeded"
            or (prior.outcome is not None and prior.outcome.kind == "not_sent" and policy.not_sent)
        ):
            try:
                return await request(identity)
            except Unavailable:
                prior = await ledger.attempt(ctx, role, identity)
                if prior is None:
                    raise
        if (
            prior.outcome is None
            or prior.outcome.kind != "rejected"
            or prior.outcome.http_status != 429
            or index == policy.rate_limit_retries
        ):
            # Retained unknown/reserved outcomes retain their formal recovery error.
            await ledger.recall(ctx, role, identity)
            raise RuntimeError("A rejected model call unexpectedly became reusable")
        delay = prior.outcome.retry_after_seconds
        delay = policy.rate_limit_wait_seconds if delay is None else delay
        if delay > policy.max_retry_after_seconds:
            await ledger.recall(ctx, role, identity)
            raise RuntimeError("Rate-limit wait exceeds the configured bound")
        following = f"{invocation_id}:rate-limit:{index + 1}"
        if await ledger.attempt(ctx, role, following) is None:
            await asyncio.sleep(delay)
    raise RuntimeError("Rate-limit retry directory is empty")
