"""Application operations for inspecting this runtime's background work."""

from collections.abc import Awaitable, Callable

from pydantic import BaseModel, ConfigDict

from nous.application.contracts import BasicEvents, CallContext, define_operation
from nous.common.errors import NousError

from .execution import ExecutionManager, RunningOperation, RunningOperationRef


class ListOperations(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class SelectOperation(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    ref: RunningOperationRef | str


LIST_OPERATIONS = define_operation(
    "runtime.operations.list",
    request=ListOperations,
    result=tuple[RunningOperation, ...],
    events=BasicEvents,
    description="List recent operations in this running instance.",
)
READ_OPERATION = define_operation(
    "runtime.operations.read",
    request=SelectOperation,
    result=RunningOperation,
    events=BasicEvents,
    description="Read the current state of a running operation.",
)
WAIT_OPERATION = define_operation(
    "runtime.operations.wait",
    request=SelectOperation,
    result=RunningOperation,
    events=BasicEvents,
    description="Wait for a running operation to finish.",
)
CANCEL_OPERATION = define_operation(
    "runtime.operations.cancel",
    request=SelectOperation,
    result=RunningOperation,
    events=BasicEvents,
    description="Cancel a running operation and wait for its cleanup.",
)
CHECK_RUNTIME = define_operation(
    "runtime.check",
    request=ListOperations,
    result=dict[str, object],
    events=BasicEvents,
    description="Inspect the resources of this instance.",
)


class RuntimeOperations:
    def __init__(
        self, execution: ExecutionManager, check: Callable[[], Awaitable[dict[str, object]]]
    ) -> None:
        self.execution, self._check = execution, check

    def _resolve(self, ref: RunningOperationRef | str) -> RunningOperationRef:
        if not isinstance(ref, str):
            return ref
        for operation in self.execution.list_operations():
            if operation.ref.operation_id == ref:
                return operation.ref
        raise NousError("operation.expired", "This runtime operation is no longer available.")

    async def list(
        self, request: ListOperations, context: CallContext
    ) -> tuple[RunningOperation, ...]:
        return self.execution.list_operations()

    async def read(self, request: SelectOperation, context: CallContext) -> RunningOperation:
        return self.execution.read_operation(self._resolve(request.ref))

    async def wait(self, request: SelectOperation, context: CallContext) -> RunningOperation:
        return await self.execution.wait(self._resolve(request.ref))

    async def cancel(self, request: SelectOperation, context: CallContext) -> RunningOperation:
        return await self.execution.cancel(self._resolve(request.ref))

    async def check(self, request: ListOperations, context: CallContext) -> dict[str, object]:
        return await self._check()
