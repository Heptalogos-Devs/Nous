"""Owned native work exposes structured failures at the retrieval boundary."""

from collections.abc import Callable

from nous_native import CancellationToken, NativeError
from pydantic import JsonValue, TypeAdapter

from nous.platform.concurrency import owned_thread
from nous.platform.errors import NousError


def failure(error: NativeError, phase: str) -> NousError:
    return NousError(
        "native_retrieval_error",
        str(error),
        status=500,
        context={
            "phase": phase,
            "native_code": error.code,
            "native_details": TypeAdapter(dict[str, JsonValue]).validate_json(error.details_json),
        },
    )


async def run_native[**Params, Result](
    phase: str,
    token: CancellationToken | None,
    function: Callable[Params, Result],
    *args: Params.args,
    **kwargs: Params.kwargs,
) -> Result:
    try:
        return await owned_thread(
            lambda: function(*args, **kwargs), on_cancel=token.cancel if token else None
        )
    except NativeError as error:
        raise failure(error, phase) from error
