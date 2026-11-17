"""Complete Prompt resource operations shared by all entrances."""

import tempfile
from pathlib import Path, PurePosixPath
from typing import Literal, TypedDict, cast

from pydantic import BaseModel, ConfigDict, JsonValue

from nous.application.contracts import BasicEvents, CallContext, define_operation
from nous.common.errors import NousError
from nous.content.contracts import ExternalInput
from nous.models.prompts import PromptCatalog, PromptContext
from nous.runtime.execution import ExecutionManager


class PromptRequest(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    prompt: str


class ListPrompts(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class ReadPrompt(PromptRequest):
    origin: Literal["selected", "package", "instance"] = "selected"


class DescribePrompt(PromptRequest):
    task: str | None = None


class ImportPrompt(PromptRequest):
    files: dict[str, ExternalInput]


class PromptResourceView(TypedDict):
    id: str
    contract_id: str
    version: str
    execution_signature: str
    documentation_signature: str
    origin: str
    manifest: dict[str, JsonValue]
    templates: dict[str, str]


class PromptListView(TypedDict):
    prompts: list[PromptResourceView]


class PromptDiffView(TypedDict):
    id: str
    left: str
    right: str
    instance_override: bool
    changes: dict[str, str]
    execution_changed: bool
    documentation_changed: bool


class PromptValidationView(TypedDict):
    id: str
    version: str
    valid: bool
    contract_id: str
    templates: list[str]
    execution_signature: str
    documentation_signature: str


class PromptEditView(TypedDict):
    directory: str
    templates: list[str]


class PromptImportView(PromptValidationView):
    directory: str


PREPARE_PROMPT_EDIT = define_operation(
    "prompt.prepare_edit",
    request=PromptRequest,
    result=PromptEditView,
    events=BasicEvents,
    description="Prepare a prompt resource for editing.",
)
LIST_PROMPTS = define_operation(
    "prompt.list",
    request=ListPrompts,
    result=PromptListView,
    events=BasicEvents,
    description="List available prompt resources.",
)
READ_PROMPT = define_operation(
    "prompt.read",
    request=ReadPrompt,
    result=PromptResourceView,
    events=BasicEvents,
    description="Read a prompt resource and its templates.",
)
DESCRIBE_PROMPT = define_operation(
    "prompt.describe",
    request=DescribePrompt,
    result=PromptContext,
    description="Describe a prompt and its configured task context.",
)
DIFF_PROMPT = define_operation(
    "prompt.diff",
    request=PromptRequest,
    result=PromptDiffView,
    events=BasicEvents,
    description="Compare an instance prompt with its packaged resource.",
)
VALIDATE_PROMPT = define_operation(
    "prompt.validate",
    request=PromptRequest,
    result=PromptValidationView,
    events=BasicEvents,
    description="Validate a prompt resource and its templates.",
)
RESTORE_PROMPT = define_operation(
    "prompt.restore",
    request=PromptRequest,
    result=PromptResourceView,
    events=BasicEvents,
    description="Restore the packaged prompt resource.",
)
IMPORT_PROMPT = define_operation(
    "prompt.import",
    request=ImportPrompt,
    result=PromptImportView,
    events=BasicEvents,
    description="Import a prompt resource and its templates.",
)


class PromptCalls:
    """Own prompt inspection, editing and import for a running or offline instance."""

    def __init__(
        self,
        catalog: PromptCatalog,
        execution: ExecutionManager,
        *,
        temp: Path | None = None,
    ) -> None:
        self.prompts = catalog
        self.execution, self.temp = execution, temp

    async def describe_prompt(self, request: DescribePrompt, context: CallContext) -> PromptContext:
        return self.prompts.describe(
            request.prompt, task_assignment=request.task, snapshot=context.settings
        )

    async def prepare_prompt_edit(
        self, request: PromptRequest, context: CallContext
    ) -> PromptEditView:
        catalog = self.prompts

        def prepare() -> PromptEditView:
            directory = catalog.editable_directory(request.prompt, snapshot=context.settings)
            return {
                "directory": str(directory),
                "templates": list(catalog.get(request.prompt).templates),
            }

        return await self.execution.blocking(prepare)

    async def list_prompts(self, request: ListPrompts, context: CallContext) -> PromptListView:
        catalog = self.prompts
        return {
            "prompts": cast(list[PromptResourceView], await self.execution.blocking(catalog.list))
        }

    async def read_prompt(self, request: ReadPrompt, context: CallContext) -> PromptResourceView:
        catalog = self.prompts
        return cast(
            PromptResourceView,
            (
                await self.execution.blocking(catalog.get, request.prompt, origin=request.origin)
            ).to_dict(),
        )

    async def diff_prompt(self, request: PromptRequest, context: CallContext) -> PromptDiffView:
        catalog = self.prompts
        return cast(PromptDiffView, await self.execution.blocking(catalog.diff, request.prompt))

    async def validate_prompt(
        self, request: PromptRequest, context: CallContext
    ) -> PromptValidationView:
        catalog = self.prompts
        return cast(
            PromptValidationView, await self.execution.blocking(catalog.validate, request.prompt)
        )

    async def restore_prompt(
        self, request: PromptRequest, context: CallContext
    ) -> PromptResourceView:
        catalog = self.prompts
        return cast(
            PromptResourceView,
            (await self.execution.blocking(catalog.restore_package, request.prompt)).to_dict(),
        )

    async def import_prompt(self, request: ImportPrompt, context: CallContext) -> PromptImportView:
        if self.temp is None:
            raise NousError("operation.unavailable", "Prompt import is unavailable")
        limit = context.settings.values.content.text_read_max_bytes
        files: dict[str, str] = {}
        total = 0
        for name, source in request.files.items():
            relative = PurePosixPath(name)
            if (
                not name
                or relative.is_absolute()
                or ".." in relative.parts
                or "\\" in name
                or Path(name).is_absolute()
            ):
                raise NousError(
                    "prompt.invalid_path", "Prompt paths must be relative resource paths"
                )
            selected = await context.inputs.read_text(source.key, source.selection, limit)
            total += len(selected.data)
            if total > limit:
                raise NousError(
                    "content.too_large", "Prompt input exceeds the configured read limit"
                )
            files[name] = selected.text

        def adopt() -> PromptImportView:
            assert self.temp is not None
            self.temp.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(prefix="prompt-import-", dir=self.temp) as directory:
                root = Path(directory)
                for name, content in files.items():
                    target = root.joinpath(*PurePosixPath(name).parts)
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(content, encoding="utf-8")
                catalog = self.prompts
                target = catalog.import_directory(request.prompt, root)
                catalog.write_context(request.prompt, snapshot=context.settings)
                return cast(
                    PromptImportView, {"directory": str(target), **catalog.validate(request.prompt)}
                )

        return await self.execution.blocking(adopt)
