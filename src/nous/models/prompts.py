"""Versioned local prompt resources and deterministic Jinja compilation."""

from __future__ import annotations

import difflib
import hashlib
import json
import shutil
import tomllib
from collections import OrderedDict
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from importlib.resources import files
from importlib.resources.abc import Traversable
from pathlib import Path, PurePosixPath
from types import MappingProxyType
from typing import TYPE_CHECKING, Literal, cast

from jinja2 import (
    DictLoader,
    Environment,
    FileSystemBytecodeCache,
    StrictUndefined,
    TemplateError,
    meta,
)
from pydantic import BaseModel, ConfigDict, JsonValue, ValidationError
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    TextPart,
    UserContent,
    UserPromptPart,
)

from nous.common.errors import NousError
from nous.content.input_access import current_input_access
from nous.media.contracts import PreparedSource
from nous.presentation import translate

from .configuration import ModelRegistry
from .contracts import ReadingInput
from .tasks import TaskCatalog

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot

    from .configuration import TaskAssignment


def _signature(value: object) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, ensure_ascii=False).encode()
    ).hexdigest()


def _source_context(source: PreparedSource, language: str) -> str:
    """Render only known source scope and role next to the material the model receives."""
    lines: list[str] = []
    roles = {
        "reference": "Reference material",
        "current_basis": "Current basis",
        "inspiration": "Inspiration",
        "revision_base": "Base text",
        "edit_target": "Editable text",
        "context": "Context",
    }
    if source.group is not None and source.group.role in roles:
        role = translate(roles[source.group.role], language)
        lines.append(translate("Source role: {role}.", language).format(role=role))
    context = source.group.context if source.group else None
    if context is None:
        return "\n".join(lines)

    def seconds(value: int) -> str:
        return f"{value / 1000:.3f}".rstrip("0").rstrip(".")

    representations: set[str] = (
        {part.representation for part in source.group.parts} if source.group else set()
    )
    if context.kind in {"audio", "video"}:
        kind = translate("Video" if context.kind == "video" else "Audio", language)
        if context.duration_ms is not None:
            lines.append(
                translate(
                    "Source {kind} duration: approximately {duration} seconds.", language
                ).format(kind=kind, duration=seconds(context.duration_ms))
            )
        content = translate(
            "Transcript"
            if "transcript" in representations
            else "Audio"
            if "audio" in representations or context.kind == "audio"
            else "Video",
            language,
        )
        for selection in context.selections:
            start = selection.start_ms if selection.start_ms is not None else 0
            end = selection.end_ms if selection.end_ms is not None else context.duration_ms
            if "sampled_frame" in representations:
                lines.append(
                    translate(
                        "Provided frame from the source video at {time} seconds.", language
                    ).format(time=seconds(start))
                )
            elif end is not None:
                lines.append(
                    translate(
                        "Provided {content}: source interval {start}–{end} seconds.", language
                    ).format(content=content, start=seconds(start), end=seconds(end))
                )
            elif selection.start_ms is not None:
                lines.append(
                    translate(
                        "Provided {content} begins at {start} seconds in the source.", language
                    ).format(content=content, start=seconds(start))
                )
        if context.kind == "video" and "native_video" in representations:
            if context.audio == "excluded":
                lines.append(translate("The source audio was excluded from the video.", language))
            elif context.audio == "absent":
                lines.append(translate("The source video has no audio.", language))
    elif context.kind == "pdf":
        if context.page_count is not None:
            lines.append(
                translate("Source document has {count} pages.", language).format(
                    count=context.page_count
                )
            )
        pages = tuple(dict.fromkeys(page for item in context.selections for page in item.pages))
        if pages:
            lines.append(
                translate("Provided pages: {pages}.", language).format(
                    pages=", ".join(map(str, pages))
                )
            )
    elif context.kind == "image":
        if (
            any(selection.crop is not None for selection in context.selections)
            and context.width is not None
            and context.height is not None
        ):
            lines.append(
                translate(
                    "Oriented source image size: {width} × {height} pixels.", language
                ).format(width=context.width, height=context.height)
            )
        for selection in context.selections:
            if selection.frame is not None and context.frame_count and context.frame_count > 1:
                lines.append(
                    translate("Provided frame {frame} of {count} (zero-based).", language).format(
                        frame=selection.frame, count=context.frame_count
                    )
                )
            if selection.crop is not None:
                left, top, right, bottom = selection.crop
                lines.append(
                    translate(
                        "Image crop in oriented source pixels: ({left}, {top})–({right}, {bottom}).",
                        language,
                    ).format(left=left, top=top, right=right, bottom=bottom)
                )
    elif context.kind == "text":
        for selection in context.selections:
            start, end = selection.start_byte, selection.end_byte
            if (
                start is not None
                and end is not None
                and (start != 0 or context.byte_length is None or end != context.byte_length)
            ):
                lines.append(translate("Provided text is an excerpt of the source.", language))
                break
    return "\n".join(lines)


class PromptDocumentation(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    title: str = ""
    purpose: str = ""
    consumers: tuple[str, ...] = ()
    input_notes: str = ""
    output_notes: str = ""


class MessageTemplate(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    role: Literal["system", "user", "assistant"]
    template: str
    append_sources: bool = False
    description: str = ""


class PromptManifest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    contract_id: str
    messages: tuple[MessageTemplate, ...]
    documentation: PromptDocumentation = PromptDocumentation()


class PromptContext(BaseModel):
    """Human editing context captured from the resource, task and effective configuration."""

    resource: dict[str, JsonValue]
    contract: dict[str, JsonValue]
    assignments: list[dict[str, JsonValue]]
    input_state: str = "No request input supplied; media and context window are not prepared."

    def to_dict(self) -> dict[str, JsonValue]:
        return self.model_dump(mode="json")


@dataclass(frozen=True)
class PromptResource:
    """Loaded manifest and literal dependencies, identified together by their content digest."""

    id: str
    manifest: PromptManifest
    templates: Mapping[str, str]
    manifest_text: str
    version: str
    origin: str
    documentation_signature: str

    @property
    def execution_signature(self) -> str:
        return self.version

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            "id": self.id,
            "contract_id": self.manifest.contract_id,
            "version": self.version,
            "execution_signature": self.execution_signature,
            "documentation_signature": self.documentation_signature,
            "origin": self.origin,
            "manifest": self.manifest.model_dump(mode="json"),
            "templates": dict(self.templates),
        }


@dataclass(frozen=True)
class CompiledPrompt:
    resource: PromptResource
    messages: tuple[ModelMessage, ...]
    preview: tuple[dict[str, JsonValue], ...]
    reading: ReadingInput
    schema: dict[str, JsonValue] | None
    validate_output: Callable[[JsonValue, ReadingInput], JsonValue]

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            "prompt_id": self.resource.id,
            "contract_id": self.resource.manifest.contract_id,
            "template_version": self.resource.version,
            "execution_signature": self.resource.execution_signature,
            "documentation_signature": self.resource.documentation_signature,
            "messages": list(self.preview),
            "source_groups": [
                source.group.model_dump(mode="json")
                for source in self.reading.sources
                if source.group is not None
            ],
            "schema": self.schema,
            "schema_version": hashlib.sha256(
                json.dumps(self.schema, sort_keys=True).encode()
            ).hexdigest()
            if self.schema
            else None,
        }


class PromptCatalog:
    """Select an instance override or packaged resource and capture its template dependencies."""

    def __init__(self, config_directory: Path, tasks: TaskCatalog) -> None:
        self.directory = config_directory / "prompts"
        self.tasks = tasks

    @staticmethod
    def _safe_name(name: str) -> str:
        path = PurePosixPath(name)
        if not name or path.is_absolute() or ".." in path.parts or "\\" in name:
            raise NousError("prompt.invalid_path", "Prompt paths must be relative resource paths")
        return name

    def _root(self, prompt_id: str):
        self._safe_name(prompt_id)
        if "/" in prompt_id:
            raise NousError("prompt.invalid_id", "Prompt ID must be a single directory name")
        local = self.directory / prompt_id
        return (
            (local, "instance")
            if local.is_dir()
            else (files("nous.models.resources").joinpath(prompt_id), "package")
        )

    def list(self) -> list[dict[str, JsonValue]]:
        package = files("nous.models.resources")
        ids = {
            p.name for p in package.iterdir() if p.is_dir() and p.joinpath("prompt.toml").is_file()
        }
        if self.directory.exists():
            ids.update(
                p.name
                for p in self.directory.iterdir()
                if p.is_dir() and (p / "prompt.toml").is_file()
            )
        return [self.get(name).to_dict() for name in sorted(ids)]

    def get(
        self, prompt_id: str, *, origin: Literal["selected", "package", "instance"] = "selected"
    ) -> PromptResource:
        """Read each dependency once and include the task contract in the resource version."""
        root, selected = self._root(prompt_id)
        if origin == "package":
            root, selected = files("nous.models.resources").joinpath(prompt_id), "package"
        elif origin == "instance":
            root, selected = self.directory / prompt_id, "instance"
        return self._load(prompt_id, root, selected)

    def _load(
        self,
        prompt_id: str,
        root: Traversable,
        origin: str,
        read_text: Callable[[Traversable], str] | None = None,
    ) -> PromptResource:
        """Capture one resource directly from its declared directory."""

        def default_read(path: Traversable) -> str:
            return path.read_text(encoding="utf-8")

        read = read_text or default_read
        try:
            manifest_text = read(root.joinpath("prompt.toml"))
            manifest = PromptManifest.model_validate(tomllib.loads(manifest_text))
            if manifest.contract_id not in self.tasks:
                raise NousError(
                    "prompt.unknown_contract", f"Unknown task contract: {manifest.contract_id}"
                )
            if not manifest.messages:
                raise NousError("prompt.empty", "Prompt requires at least one message")
            templates: dict[str, str] = {}
            parser = Environment(undefined=StrictUndefined, autoescape=False)
            pending = [item.template for item in manifest.messages]
            while pending:
                name = self._safe_name(pending.pop())
                if name in templates:
                    continue
                source = read(root.joinpath(name))
                templates[name] = source
                parsed = parser.parse(source)
                unknown = (
                    meta.find_undeclared_variables(parsed) - ReadingInput().template_values().keys()
                )
                if unknown:
                    raise NousError(
                        "prompt.unknown_variable", f"Unknown prompt variables: {sorted(unknown)}"
                    )
                for reference in meta.find_referenced_templates(parsed):
                    if reference is None:
                        raise NousError(
                            "prompt.dynamic_include",
                            "Dynamic template dependencies are unsupported",
                        )
                    pending.append(reference)
            for message in manifest.messages:
                if message.append_sources and message.role != "user":
                    raise NousError("prompt.invalid_source_slot", "Sources require a user message")
            contract = self.tasks[manifest.contract_id].describe_contract()
            variables = cast(dict[str, dict[str, JsonValue]], contract["variables"])
            execution_contract = self.tasks[manifest.contract_id].execution_contract()
            execution = manifest.model_dump(mode="json", exclude={"documentation"})
            execution["messages"] = [
                item.model_dump(mode="json", exclude={"description"}) for item in manifest.messages
            ]
            digest = _signature([execution, templates, execution_contract])
            documentation = _signature(
                [
                    manifest.documentation.model_dump(mode="json"),
                    [item.description for item in manifest.messages],
                    {name: info.get("description") for name, info in variables.items()},
                    contract["named_outputs"],
                ]
            )
            return PromptResource(
                prompt_id,
                manifest,
                MappingProxyType(templates),
                manifest_text,
                digest,
                origin,
                documentation,
            )
        except NousError:
            raise
        except (OSError, ValueError, ValidationError, TemplateError) as error:
            raise NousError("prompt.invalid", f"Cannot load prompt {prompt_id}: {error}") from error

    def validate(self, prompt_id: str) -> dict[str, JsonValue]:
        resource = self.get(prompt_id)
        return {
            "id": prompt_id,
            "version": resource.version,
            "valid": True,
            "contract_id": resource.manifest.contract_id,
            "templates": list(resource.templates),
            "execution_signature": resource.execution_signature,
            "documentation_signature": resource.documentation_signature,
        }

    def describe(
        self,
        prompt_id: str,
        *,
        task_assignment: str | TaskAssignment | None = None,
        snapshot: ConfigSnapshot | None = None,
    ) -> PromptContext:
        """Describe a prompt, its task contract and optional configured assignments.

        Supplying a task assignment requires a snapshot and the selected task
        must reference this prompt. Without an explicit assignment, a snapshot
        includes all tasks configured to use the prompt.
        """
        resource = self.get(prompt_id)
        assignments: list[dict[str, JsonValue]] = []
        if snapshot is not None:
            config = snapshot.values
            if isinstance(task_assignment, str):
                if task_assignment not in config.tasks:
                    raise NousError("model.task_missing", f"Unknown task: {task_assignment}")
                chosen = [(task_assignment, config.tasks[task_assignment])]
            elif task_assignment is not None:
                chosen = [("explicit", task_assignment)]
            else:
                chosen = [
                    (name, item) for name, item in config.tasks.items() if item.prompt == prompt_id
                ]
            for name, assignment in chosen:
                if assignment.prompt != prompt_id:
                    raise NousError("prompt.task_mismatch", f"Task {name} uses another prompt")
                models: list[JsonValue] = []
                for route in ModelRegistry().routes(assignment, snapshot):
                    model = route.model
                    provider = config.providers.get(model.provider)
                    models.append(
                        {
                            "id": route.model_id,
                            "configuration": model.model_dump(
                                mode="json", exclude={"compatibility"}
                            ),
                            "preset": route.preset.model_dump(mode="json", exclude_none=True),
                            "protocol": provider.protocol if provider else None,
                        }
                    )
                assignments.append(
                    {
                        "task": name,
                        "assignment": assignment.model_dump(mode="json"),
                        "models": models,
                        "execution_limits": config.model_execution.model_dump(mode="json"),
                        "configuration_version": snapshot.version,
                    }
                )
        elif task_assignment is not None:
            raise NousError(
                "prompt.snapshot_required", "Task context requires a configuration snapshot"
            )
        return PromptContext(
            resource=resource.to_dict(),
            contract=self.tasks[resource.manifest.contract_id].describe_contract(),
            assignments=assignments,
        )

    def diff(
        self,
        prompt_id: str,
        *,
        left: Literal["package", "instance"] = "package",
        right: Literal["package", "instance"] = "instance",
    ) -> dict[str, JsonValue]:
        package = self.get(prompt_id, origin="package")
        if not (self.directory / prompt_id).is_dir():
            return {
                "id": prompt_id,
                "left": left,
                "right": right,
                "instance_override": False,
                "changes": {},
                "execution_changed": False,
                "documentation_changed": False,
            }
        resources = [
            package if side == "package" else self.get(prompt_id, origin=side)
            for side in (left, right)
        ]
        contents = [{"prompt.toml": item.manifest_text, **item.templates} for item in resources]
        changes: dict[str, JsonValue] = {}
        for name in sorted(contents[0].keys() | contents[1].keys()):
            delta = "".join(
                difflib.unified_diff(
                    contents[0].get(name, "").splitlines(keepends=True),
                    contents[1].get(name, "").splitlines(keepends=True),
                    fromfile=f"{left}/{name}",
                    tofile=f"{right}/{name}",
                )
            )
            if delta:
                changes[name] = delta
        return {
            "id": prompt_id,
            "left": left,
            "right": right,
            "instance_override": True,
            "changes": changes,
            "execution_changed": resources[0].version != resources[1].version,
            "documentation_changed": resources[0].documentation_signature
            != resources[1].documentation_signature,
        }

    def restore_package(self, prompt_id: str) -> PromptResource:
        """Remove the instance override after validating the packaged resource.

        Returns the now-selected packaged resource. The override directory and
        its contents are removed; a symlink override is unlinked.
        """
        # Resolve and validate the package before removing the explicitly selected override.
        self.get(prompt_id, origin="package")
        target = self.directory / prompt_id
        if target.is_symlink():
            target.unlink()
        elif target.exists():
            shutil.rmtree(target)
        return self.get(prompt_id)

    def write_context(self, prompt_id: str, *, snapshot: ConfigSnapshot | None = None) -> Path:
        context = self.describe(prompt_id, snapshot=snapshot)
        target = self.directory / prompt_id
        target.mkdir(parents=True, exist_ok=True)
        (target / "input-contract.json").write_text(
            json.dumps(context.contract.get("input"), ensure_ascii=False, indent=2),
            encoding="utf-8",
        )
        (target / "output-schema.json").write_text(
            json.dumps(context.contract.get("output"), ensure_ascii=False, indent=2),
            encoding="utf-8",
        )
        resource = context.resource
        manifest = cast(dict[str, JsonValue], resource["manifest"])
        documentation = cast(dict[str, JsonValue], manifest["documentation"])
        messages = cast(list[dict[str, JsonValue]], manifest["messages"])
        overview = "\n".join(
            f"- `{message['role']}` / `{message['template']}`: {message['description']}"
            for message in messages
        )
        assignments: list[str] = []
        for configured in context.assignments:
            assignment = cast(dict[str, JsonValue], configured["assignment"])
            assignments.append(
                f"### {configured['task']}\n\n"
                f"输出模式：`{assignment['output_mode']}`；预设：`{assignment.get('preset') or '默认'}`。"
            )
            for index, entry in enumerate(cast(list[dict[str, JsonValue]], configured["models"])):
                model = cast(dict[str, JsonValue], entry["configuration"])
                preset = json.dumps(entry["preset"], ensure_ascii=False, sort_keys=True)
                assignments.append(
                    f"- {'首选' if index == 0 else '后备'} `{entry['id']}` "
                    f"({model['model_name']})：协议 `{entry['protocol']}`，"
                    f"能力 {', '.join(cast(list[str], model['capabilities']))}，"
                    f"输入格式 {', '.join(cast(list[str], model['input_formats']))}；"
                    f"预设参数：`{preset}`。"
                )
            limits = json.dumps(configured["execution_limits"], ensure_ascii=False, sort_keys=True)
            assignments.append(f"- 实例执行限制：`{limits}`。")
        (target / "CONTEXT.md").write_text(
            f"# {documentation['title'] or prompt_id}\n\n"
            f"{documentation['purpose']}\n\n"
            f"资源：`{prompt_id}`；当前来源：`{resource['origin']}`。\n\n"
            f"## 模型输入\n\n{documentation['input_notes']}\n\n"
            f"## 模型输出\n\n{documentation['output_notes']}\n\n"
            f"## 消息与模板\n\n{overview}\n\n"
            "## 任务与模型\n\n"
            + ("\n".join(assignments) if assignments else "当前没有配置使用此提示词的任务。")
            + "\n\n[输入合同](input-contract.json) · [输出 Schema](output-schema.json)\n\n"
            "这些说明来自提示词资源、实际任务合同和当前配置，不发送给模型。"
            "实际媒体与参数请使用预览查看。\n",
            encoding="utf-8",
        )
        return target / "CONTEXT.md"

    def import_directory(self, prompt_id: str, source: Path) -> Path:
        """Validate and copy a prompt manifest and its templates to the instance.

        The returned path is the instance override directory. Existing files
        with the same names are overwritten; editing context is regenerated.
        """
        self._safe_name(prompt_id)
        if "/" in prompt_id:
            raise NousError("prompt.invalid_id", "Prompt ID must be a single directory name")
        # Validate in its current directory before publishing the instance resource.
        access = current_input_access.get()
        if access is None:
            source = source.resolve()
            candidate = self._load(prompt_id, source, "import")
        else:
            # Fetch only the declared manifest and template dependencies from
            # the client; unrelated files in the selected directory stay put.
            source = source.absolute()

            def read_client(path: Traversable) -> str:
                return access.materialize_sync(Path(str(path))).read_text(encoding="utf-8")

            candidate = self._load(prompt_id, source, "import", read_client)
        target = self.directory / prompt_id
        target.mkdir(parents=True, exist_ok=True)
        (target / "prompt.toml").write_text(candidate.manifest_text, encoding="utf-8")
        for name, contents in candidate.templates.items():
            destination = target / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(contents, encoding="utf-8")
        self.write_context(prompt_id)
        return target

    def editable_directory(self, prompt_id: str, *, snapshot: ConfigSnapshot | None = None) -> Path:
        """Return an instance directory ready for editing the selected prompt.

        A packaged resource is copied into an instance override on first edit;
        editing context is regenerated from the resource, task and snapshot.
        """
        resource = self.get(prompt_id)
        target = self.directory / prompt_id
        if resource.origin == "package":
            target.mkdir(parents=True, exist_ok=True)
            (target / "prompt.toml").write_text(resource.manifest_text, encoding="utf-8")
            for name, source in resource.templates.items():
                path = target / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(source, encoding="utf-8")
        self.write_context(prompt_id, snapshot=snapshot)
        return target


class PromptCompiler:
    """Render captured resource contents into matching SDK messages and readable previews."""

    def __init__(
        self,
        catalog: PromptCatalog,
        cache_directory: Path | None = None,
        *,
        cache_entries: int = 128,
        bytecode_cache: bool = True,
    ) -> None:
        self.catalog = catalog
        self.cache_entries = cache_entries
        self._environments: OrderedDict[str, Environment] = OrderedDict()
        self.bytecode_directory: Path | None = None
        if bytecode_cache and cache_directory is not None:
            path = cache_directory / "prompts"
            path.mkdir(parents=True, exist_ok=True)
            self.bytecode_directory = path

    def _environment(self, resource: PromptResource) -> Environment:
        environment = self._environments.get(resource.version)
        if environment is not None:
            self._environments.move_to_end(resource.version)
            return environment
        bytecode: FileSystemBytecodeCache | None = None
        if self.bytecode_directory is not None:
            # Different resources may use the same template names; isolate their bytecode by version.
            directory = self.bytecode_directory / resource.version
            directory.mkdir(parents=True, exist_ok=True)
            bytecode = FileSystemBytecodeCache(str(directory))
        environment = Environment(
            loader=DictLoader(resource.templates),
            undefined=StrictUndefined,
            autoescape=False,
            cache_size=self.cache_entries,
            bytecode_cache=bytecode,
        )
        if self.cache_entries:
            self._environments[resource.version] = environment
            while len(self._environments) > self.cache_entries:
                self._environments.popitem(last=False)
        return environment

    def compile(
        self,
        prompt_id: str | PromptResource,
        reading: ReadingInput,
        *,
        schema: dict[str, JsonValue] | None = None,
        language: str = "zh-CN",
    ) -> CompiledPrompt:
        """Render source values once; the prepared request keeps these messages for execution."""
        resource = self.catalog.get(prompt_id) if isinstance(prompt_id, str) else prompt_id
        self.catalog.tasks[resource.manifest.contract_id].validate_input(reading)
        output = self.catalog.tasks[resource.manifest.contract_id].output_type
        if schema is not None and resource.manifest.contract_id != "custom-reading":
            raise NousError(
                "model.schema_conflict", "Custom schema requires the custom-reading contract"
            )
        if resource.manifest.contract_id == "custom-reading" and schema is None:
            raise NousError("model.schema_required", "Custom reading requires a JSON Schema")
        if output:
            schema = cast(dict[str, JsonValue], output.model_json_schema())
        environment = self._environment(resource)
        values = reading.template_values()
        messages: list[ModelMessage] = []
        preview: list[dict[str, JsonValue]] = []
        for item in resource.manifest.messages:
            try:
                rendered = environment.get_template(item.template).render(values)
            except Exception as error:
                raise NousError("prompt.render_failed", str(error)) from error
            parts: list[UserContent] = [rendered]
            shown: list[JsonValue] = [{"type": "text", "text": rendered}]
            if item.append_sources:
                for source in reading.sources:
                    heading = f"\n\n[{source.key}] {source.name}"
                    context = _source_context(source, language)
                    if context:
                        heading += "\n" + context
                    if source.text is not None:
                        heading += "\n" + source.text
                    parts.append(heading)
                    parts.extend(source.media)
                    shown.append({"type": "text", "text": heading})
                    for medium in source.media:
                        shown.append(
                            {
                                "type": "media",
                                "media_type": medium.media_type,
                                "bytes": len(medium.data),
                                "source_key": source.key,
                                "metadata": source.metadata,
                            }
                        )
            if item.role == "system":
                messages.append(ModelRequest(parts=[SystemPromptPart(rendered)]))
            elif item.role == "assistant":
                messages.append(ModelResponse(parts=[TextPart(rendered)]))
            else:
                messages.append(ModelRequest(parts=[UserPromptPart(parts)]))
            preview.append({"role": item.role, "content": shown})
        keys = [source.key for source in reading.sources]
        if len(set(keys)) != len(keys):
            raise NousError("model.duplicate_source", "Source keys must be unique in one request")
        return CompiledPrompt(
            resource,
            tuple(messages),
            tuple(preview),
            reading,
            schema,
            self.catalog.tasks[resource.manifest.contract_id].validate,
        )
