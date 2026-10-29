"""Consumer-owned contracts are registered by application composition."""

from collections.abc import Callable
from dataclasses import dataclass

from nous_wave.contracts import DomainError
from pydantic import BaseModel, JsonValue


@dataclass(frozen=True)
class RoleDefinition:
    purpose: str
    variant: str
    input_type: type[BaseModel]
    output_type: type[BaseModel]
    default: bool = False
    invocation_purpose: str | None = None
    transform_prompt: Callable[[dict[str, object], BaseModel], dict[str, object]] | None = None

    def variables(self, variables: dict[str, object]) -> tuple[BaseModel, dict[str, object]]:
        parsed = self.input_type.model_validate(variables)
        return parsed, parsed.model_dump(mode="json", exclude_unset=True)


class RoleRegistry:
    def __init__(self) -> None:
        self.definitions: dict[tuple[str, str], RoleDefinition] = {}

    def register(self, definition: RoleDefinition) -> None:
        key = (definition.purpose, definition.variant)
        if key in self.definitions:
            raise ValueError(f"Duplicate role contract: {key}")
        self.definitions[key] = definition

    def variants(self, purpose: str) -> tuple[str, ...]:
        return tuple(
            value.variant for value in self.definitions.values() if value.purpose == purpose
        )

    def resolve(
        self, purpose: str, variant: str | None = None, output_type: type[BaseModel] | None = None
    ) -> RoleDefinition:
        candidates = [
            value
            for value in self.definitions.values()
            if value.purpose == purpose
            and (variant is None or value.variant == variant)
            and (output_type is None or value.output_type is output_type)
        ]
        if variant is None and output_type is None:
            candidates = [value for value in candidates if value.default]
        if len(candidates) != 1:
            raise DomainError(
                "model_contract_unavailable",
                "Role has no unambiguous registered consumer contract",
                purpose=purpose,
                variant=variant,
            )
        return candidates[0]


def json_object(value: object) -> dict[str, JsonValue]:
    from pydantic import TypeAdapter

    return TypeAdapter(dict[str, JsonValue]).validate_python(value)
