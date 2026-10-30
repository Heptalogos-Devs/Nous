"""Safe route policy, physical attempt attribution and retained outcomes."""

from typing import Literal

from .common import Contract

SafeRejection = Literal[401, 403, 404, 405, 413, 415, 422, 429]


class ModelRoutingPolicy(Contract):
    unavailable: bool = True
    not_sent: bool = True
    rejected_statuses: tuple[SafeRejection, ...] = (401, 403, 404, 405, 413, 415, 422, 429)
    unsupported_request: bool = True


class ModelCallOutcome(Contract):
    kind: Literal["not_sent", "rejected", "invalid_output", "unknown"]
    http_status: int | None = None
    provider_code: str | None = None
    provider_type: str | None = None
    parameter: str | None = None


class ModelRouteAttempt(Contract):
    intent_id: str
    route_id: str | None
    route_index: int
