"""Consumer-owned contracts are registered by application composition."""

from collections.abc import Callable
from dataclasses import dataclass

from nous_sdk.contracts import DomainError
from pydantic import BaseModel, JsonValue

from nous.platform.configuration.models import ModelConfiguration


@dataclass(frozen=True)
class RoleDefinition:
    purpose: str
    variant: str
    input_type: type[BaseModel]
    output_type: type[BaseModel] | type[str]
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

    def validate_resources(self, value: BaseModel) -> None:
        resources = ModelConfiguration.model_validate(value)
        for role in resources.roles.values():
            if role.purpose != "embedding":
                self.resolve(role.purpose, output_mode=role.output_mode)

    def resources_schema(self) -> dict[str, JsonValue]:
        schema = json_object(ModelConfiguration.model_json_schema())
        definitions = schema.get("$defs")
        role = definitions.get("RoleResource") if isinstance(definitions, dict) else None
        properties = role.get("properties") if isinstance(role, dict) else None
        purpose = properties.get("purpose") if isinstance(properties, dict) else None
        if not isinstance(purpose, dict):
            raise RuntimeError("Model resource schema has no purpose definition")
        choices: list[JsonValue] = [
            name
            for name in sorted(
                {"embedding", *(value.purpose for value in self.definitions.values())}
            )
        ]
        purpose["enum"] = choices
        return schema

    def resolve(
        self,
        purpose: str,
        variant: str | None = None,
        output_type: type[BaseModel] | type[str] | None = None,
        output_mode: str | None = None,
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
        selected = candidates[0]
        expected_mode = "text" if selected.output_type is str else "json_schema"
        if output_mode is not None and output_mode != expected_mode:
            raise DomainError(
                "model_contract_unavailable",
                "Role output mode does not match its consumer contract",
                purpose=purpose,
                variant=selected.variant,
                expected_output_mode=expected_mode,
                configured_output_mode=output_mode,
            )
        return selected


def json_object(value: object) -> dict[str, JsonValue]:
    from pydantic import TypeAdapter

    return TypeAdapter(dict[str, JsonValue]).validate_python(value)
