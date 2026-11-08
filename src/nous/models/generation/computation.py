"""Durable references for frozen language policy and successful computation."""

from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, cast

from pydantic import BaseModel, JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.content.contracts import ArtifactRef, ContentValue, InputGroup, OutputContent
from nous.media.configuration import MediaConfig
from nous.media.contracts import PreparedSource
from nous.models.configuration import (
    ModelConfig,
    ModelExecutionConfig,
    ModelRoute,
    PresetConfig,
    TaskAssignment,
)

from ..contracts import GenerationResult, ReadingInput
from ..prompts import PromptManifest, PromptResource
from ..requests import FrozenTask, UnavailableTask

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot
    from nous.content.results import ResultStore


class FrozenLanguageTask(ContentValue):
    descriptor: ArtifactRef


async def descriptor_text(results: ResultStore, ref: ArtifactRef) -> str:
    """Read a formally retained computation body using its accepted byte allowance."""
    blob = await results.artifacts.blob(ref)
    return await results.artifacts.content.read_text(blob, max_bytes=max(blob.byte_length, 1))


def reading_value(reading: ReadingInput) -> dict[str, JsonValue]:
    return {
        "question": reading.question,
        "instructions": reading.instructions,
        "sources": [
            {
                "key": source.key,
                "name": source.name,
                "text": source.text,
                "group": source.group.model_dump(mode="json") if source.group else None,
                "metadata": source.metadata,
            }
            for source in reading.sources
        ],
    }


def read_reading(value: Mapping[str, JsonValue]) -> ReadingInput:
    sources = cast(list[dict[str, JsonValue]], value["sources"])
    return ReadingInput(
        str(value["question"]),
        str(value["instructions"]),
        tuple(
            PreparedSource(
                key=str(source["key"]),
                name=str(source["name"]),
                text=cast(str | None, source["text"]),
                group=InputGroup.model_validate(source["group"]) if source["group"] else None,
                metadata=cast(dict[str, JsonValue], source["metadata"]),
            )
            for source in sources
        ),
    )


class SavedGeneration(BaseModel):
    """The model response fields shared by language and transcription recovery."""

    output: JsonValue
    producer: dict[str, JsonValue]
    usage: dict[str, JsonValue]
    request: dict[str, JsonValue]
    attempts: int
    finish_reason: str | None
    response: dict[str, JsonValue]
    coverage: dict[str, JsonValue]

    @classmethod
    def from_result(cls, generated: GenerationResult) -> SavedGeneration:
        return cls.model_validate(generated, from_attributes=True)

    def to_result(self, reading: ReadingInput | None = None) -> GenerationResult:
        return GenerationResult(
            output=self.output,
            producer=self.producer,
            usage=self.usage,
            request=self.request,
            attempts=self.attempts,
            finish_reason=self.finish_reason,
            response=self.response,
            reading=reading,
            coverage=self.coverage,
        )


def generated_value(generated: GenerationResult, reading: ReadingInput) -> dict[str, JsonValue]:
    return {
        **SavedGeneration.from_result(generated).model_dump(mode="json"),
        "reading": reading_value(reading),
    }


def read_generated(value: Mapping[str, JsonValue]) -> GenerationResult:
    return SavedGeneration.model_validate(value).to_result(
        read_reading(cast(dict[str, JsonValue], value["reading"]))
    )


async def freeze_task(
    results: ResultStore,
    frozen: FrozenTask,
    snapshot: ConfigSnapshot,
    contract: dict[str, JsonValue],
    transcription: ArtifactRef | UnavailableTask | None = None,
) -> FrozenLanguageTask:
    import json

    resource = frozen.resource
    value = {
        "task": frozen.task,
        "assignment": frozen.assignment.model_dump(mode="json"),
        "routes": [
            {
                "model_id": route.model_id,
                "model": route.model.model_dump(mode="json"),
                "preset": route.preset.model_dump(mode="json"),
                "provider": {
                    key: snapshot.values.providers[route.model.provider].model_dump(mode="json")[
                        key
                    ]
                    for key in ("endpoint", "protocol", "credential_ref")
                },
            }
            for route in frozen.routes
        ],
        "resource": {**resource.to_dict(), "manifest_text": resource.manifest_text}
        if resource
        else None,
        "contract": contract,
        "execution": snapshot.values.model_execution.model_dump(mode="json"),
        "media": snapshot.values.media.model_dump(mode="json"),
        "transcription": {"descriptor": transcription.model_dump(mode="json")}
        if isinstance(transcription, ArtifactRef)
        else {"unavailable": TypeAdapter(UnavailableTask).dump_python(transcription, mode="json")}
        if isinstance(transcription, UnavailableTask)
        else None,
    }
    result = await results.adopt(
        operation="model.freeze_language_task",
        producer={"kind": "deterministic"},
        inputs=(),
        outputs={
            "descriptor": OutputContent(
                data=json.dumps(value, ensure_ascii=False, sort_keys=True).encode(),
                media_type="application/json",
                display_name="language-task.json",
            )
        },
        dependencies=(transcription,) if isinstance(transcription, ArtifactRef) else (),
    )
    return FrozenLanguageTask(descriptor=result.outputs[0].artifact)


async def load_task(
    results: ResultStore,
    ref: ArtifactRef,
    snapshot: ConfigSnapshot,
    contracts: object,
    *,
    check_bindings: bool = True,
) -> FrozenTask:
    import json

    value = json.loads(await descriptor_text(results, ref))
    routes: list[ModelRoute] = []
    for item in value["routes"]:
        model = ModelConfig.model_validate(item["model"])
        current_model = snapshot.values.models.get(item["model_id"])
        current_provider = snapshot.values.providers.get(model.provider)
        if check_bindings and (
            current_model is None
            or current_model.model_dump(mode="json") != item["model"]
            or current_provider is None
        ):
            raise NousError(
                "model.resource_binding",
                "Frozen model resource no longer matches the installed binding",
                details={"model_id": item["model_id"]},
            )
        for key in ("endpoint", "protocol", "credential_ref"):
            if (
                check_bindings
                and current_provider is not None
                and current_provider.model_dump(mode="json")[key] != item["provider"][key]
            ):
                raise NousError(
                    "model.resource_binding",
                    "Frozen provider resource no longer matches the installed binding",
                    details={"provider": model.provider, "field": key},
                )
        routes.append(
            ModelRoute(item["model_id"], model, PresetConfig.model_validate(item["preset"]))
        )
    item = value["resource"]
    resource = (
        PromptResource(
            item["id"],
            PromptManifest.model_validate(item["manifest"]),
            item["templates"],
            item["manifest_text"],
            item["version"],
            "frozen",
            item["documentation_signature"],
        )
        if item
        else None
    )
    # The installed task interpreter must still implement the frozen contract.
    if resource is not None and check_bindings:
        from ..tasks import TaskCatalog

        catalog = cast(TaskCatalog, contracts)
        if (
            resource.manifest.contract_id not in catalog
            or catalog[resource.manifest.contract_id].execution_contract() != value["contract"]
        ):
            raise NousError(
                "model.task_contract_changed",
                "Installed task contract differs from the accepted frozen task",
            )
    transcription = value["transcription"]
    return FrozenTask(
        value["task"],
        TaskAssignment.model_validate(value["assignment"]),
        tuple(routes),
        resource,
        execution_limits=ModelExecutionConfig.model_validate(value["execution"]),
        media_settings=MediaConfig.model_validate(value["media"]),
        transcription=ArtifactRef.model_validate(transcription["descriptor"])
        if transcription is not None and "descriptor" in transcription
        else TypeAdapter(UnavailableTask).validate_python(transcription["unavailable"])
        if transcription is not None
        else None,
    )
