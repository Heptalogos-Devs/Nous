"""Versioned local prompt resources and deterministic Jinja compilation."""

from __future__ import annotations

import hashlib
import json
import tomllib
from collections import OrderedDict
from dataclasses import dataclass
from importlib.resources import files
from pathlib import Path, PurePosixPath
from typing import Literal, cast

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

from .contracts import OUTPUT_TYPES, ReadingInput


class MessageTemplate(BaseModel):
    model_config = ConfigDict(extra="forbid")
    role: Literal["system", "user", "assistant"]
    template: str
    append_sources: bool = False


class PromptManifest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    contract_id: str
    messages: list[MessageTemplate]


@dataclass(frozen=True)
class PromptResource:
    """Loaded manifest and literal dependencies, identified together by their content digest."""

    id: str
    manifest: PromptManifest
    templates: dict[str, str]
    manifest_text: str
    version: str
    origin: str

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            "id": self.id,
            "contract_id": self.manifest.contract_id,
            "version": self.version,
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

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            "prompt_id": self.resource.id,
            "contract_id": self.resource.manifest.contract_id,
            "template_version": self.resource.version,
            "messages": list(self.preview),
            "schema": self.schema,
            "schema_version": hashlib.sha256(
                json.dumps(self.schema, sort_keys=True).encode()
            ).hexdigest()
            if self.schema
            else None,
        }


class PromptCatalog:
    """Select an instance override or packaged resource and capture its template dependencies."""

    def __init__(self, config_directory: Path) -> None:
        self.directory = config_directory / "prompts"

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

    def get(self, prompt_id: str) -> PromptResource:
        """Read each dependency once and include the task contract in the resource version."""
        root, origin = self._root(prompt_id)
        try:
            manifest_text = root.joinpath("prompt.toml").read_text(encoding="utf-8")
            manifest = PromptManifest.model_validate(tomllib.loads(manifest_text))
            if manifest.contract_id not in OUTPUT_TYPES:
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
                source = root.joinpath(name).read_text(encoding="utf-8")
                templates[name] = source
                parsed = parser.parse(source)
                unknown = meta.find_undeclared_variables(parsed) - {
                    "question",
                    "instructions",
                    "sources",
                }
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
            output = OUTPUT_TYPES[manifest.contract_id]
            contract = {
                "input": "ReadingInput-v1",
                "output": output.model_json_schema() if output else {"type": "string"},
            }
            digest = hashlib.sha256(
                json.dumps(
                    [manifest_text, templates, contract], sort_keys=True, ensure_ascii=False
                ).encode()
            ).hexdigest()
            return PromptResource(prompt_id, manifest, templates, manifest_text, digest, origin)
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
        }

    def import_directory(self, prompt_id: str, source: Path) -> Path:
        self._safe_name(prompt_id)
        if "/" in prompt_id:
            raise NousError("prompt.invalid_id", "Prompt ID must be a single directory name")
        # Validate in its current directory before publishing the instance resource.
        source = source.resolve()
        candidate_catalog = PromptCatalog(source.parent)
        candidate_catalog.directory = source.parent
        candidate = candidate_catalog.get(source.name)
        target = self.directory / prompt_id
        target.mkdir(parents=True, exist_ok=True)
        (target / "prompt.toml").write_text(candidate.manifest_text, encoding="utf-8")
        for name, contents in candidate.templates.items():
            destination = target / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(contents, encoding="utf-8")
        return target

    def editable_directory(self, prompt_id: str) -> Path:
        resource = self.get(prompt_id)
        target = self.directory / prompt_id
        if resource.origin == "package":
            target.mkdir(parents=True, exist_ok=True)
            (target / "prompt.toml").write_text(resource.manifest_text, encoding="utf-8")
            for name, source in resource.templates.items():
                path = target / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(source, encoding="utf-8")
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
        self, prompt_id: str, reading: ReadingInput, *, schema: dict[str, JsonValue] | None = None
    ) -> CompiledPrompt:
        """Render source values once; the prepared request keeps these messages for execution."""
        resource = self.catalog.get(prompt_id)
        output = OUTPUT_TYPES[resource.manifest.contract_id]
        if schema is not None and resource.manifest.contract_id != "custom-reading":
            raise NousError(
                "model.schema_conflict", "Custom schema requires the custom-reading contract"
            )
        if resource.manifest.contract_id == "custom-reading" and schema is None:
            raise NousError("model.schema_required", "Custom reading requires a JSON Schema")
        if output:
            schema = cast(dict[str, JsonValue], output.model_json_schema())
        environment = self._environment(resource)
        values = {
            "question": reading.question,
            "instructions": reading.instructions,
            "sources": [{"key": s.key, "name": s.name, "text": s.text} for s in reading.sources],
        }
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
        return CompiledPrompt(resource, tuple(messages), tuple(preview), reading, schema)
