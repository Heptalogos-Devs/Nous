"""Parse explicit user times using the current request's configuration."""

from pydantic import BaseModel, ConfigDict

from nous.application.contracts import CallContext, define_operation
from nous.common.errors import NousError
from nous.temporal.values import TemporalValue, parse_explicit_time


class ParseTime(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    value: str
    domain: str = "utc"
    timezone: str | None = None
    fold: int | None = None


class ParsedTime(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    input: str
    domain: str
    timezone: str
    value: TemporalValue


PARSE_TIME = define_operation(
    "time.parse",
    request=ParseTime,
    result=ParsedTime,
    description="Parse an explicit time in a registered domain.",
)


async def parse_time(request: ParseTime, context: CallContext) -> ParsedTime:
    snapshot = context.settings
    if request.domain not in snapshot.values.time.domains:
        raise NousError("time.domain_unknown", f"Unknown time domain: {request.domain}")
    zone = request.timezone or snapshot.values.localization.timezone
    value = parse_explicit_time(
        request.value, domain=request.domain, timezone=zone, fold=request.fold
    )
    return ParsedTime(input=request.value, domain=request.domain, timezone=zone, value=value)
