"""Inspect retained provider output and reparse the current contract without changing call facts."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.identity import IdentityPurpose
from nous_sdk.contracts.model_calls import RetainedModelOutput
from openai.types.chat import ChatCompletion
from pydantic import JsonValue, TypeAdapter, ValidationError
from pydantic_ai.messages import ModelMessagesTypeAdapter, ModelResponse

from nous.identity.access import require_namespace_in

if TYPE_CHECKING:
    from . import ModelService


async def retained_output(
    self: ModelService, ctx: CallContext, call_id: str, *, purpose: IdentityPurpose = "active"
) -> RetainedModelOutput:
    async with self.ledger.authority.read(ctx) as view:
        await require_namespace_in(view, ctx, purpose=purpose)
        call = (await self.ledger.export_in(view, ctx, (call_id,)))[0]
    if call.status == "purged":
        raise DomainError(
            "model_call_unavailable", "Model output was permanently erased", call_id=call_id
        )
    usage = call.usage or {}
    raw: str | None = None
    finish = usage.get("finish_reason")
    finish = finish if isinstance(finish, str) else None
    errors: list[dict[str, JsonValue]] = []
    messages = usage.get("messages")
    if isinstance(messages, list):
        try:
            responses = [
                message
                for message in ModelMessagesTypeAdapter.validate_python(messages)
                if isinstance(message, ModelResponse)
            ]
            if responses:
                raw = responses[-1].text
                finish = responses[-1].finish_reason
        except ValidationError as error:
            errors.extend(
                TypeAdapter(list[dict[str, JsonValue]]).validate_json(
                    error.json(include_input=False, include_url=False)
                )
            )
    response = usage.get("response")
    if isinstance(response, dict):
        try:
            completion = ChatCompletion.model_validate(response)
            if completion.choices:
                raw = completion.choices[0].message.content
                finish = completion.choices[0].finish_reason
        except ValidationError as error:
            errors.extend(
                TypeAdapter(list[dict[str, JsonValue]]).validate_json(
                    error.json(include_input=False, include_url=False)
                )
            )
    raw_json: JsonValue = None
    if raw is not None:
        try:
            raw_json = TypeAdapter(JsonValue).validate_json(raw)
        except ValidationError:
            pass
    definitions = [
        definition
        for definition in self.roles.definitions.values()
        if (definition.invocation_purpose or definition.purpose) == call.purpose
    ]
    schema = call.request.get("schema")
    if len(definitions) > 1 and isinstance(schema, dict):
        definitions = [
            definition
            for definition in definitions
            if TypeAdapter(definition.output_type).json_schema().get("title") == schema.get("title")
        ]
    parsed: JsonValue = None
    variant = None
    if len(definitions) == 1:
        definition = definitions[0]
        variant = definition.variant
        try:
            adapter = TypeAdapter(definition.output_type)
            value = (
                adapter.validate_python(raw)
                if definition.output_type is str and raw is not None
                else adapter.validate_json(raw)
                if raw is not None
                else adapter.validate_python(call.result)
            )
            parsed = TypeAdapter(JsonValue).validate_python(adapter.dump_python(value, mode="json"))
        except ValidationError as error:
            errors.extend(
                TypeAdapter(list[dict[str, JsonValue]]).validate_json(
                    error.json(include_input=False, include_url=False)
                )
            )
    else:
        errors.append(
            {
                "type": "model_contract_unavailable",
                "msg": "No unambiguous registered output contract matches this physical request",
            }
        )
    complete = finish == "stop"
    if not complete:
        errors.append(
            {
                "type": "model_output_incomplete",
                "msg": "The retained provider output has no completed stop",
                "finish_reason": finish,
            }
        )
    role = call.request.get("budget_role")
    return RetainedModelOutput(
        call_id=call.id,
        status=call.status,
        role=role if isinstance(role, str) else call.purpose,
        variant=variant,
        finish_reason=finish,
        complete=complete,
        raw_text=raw,
        raw_json=raw_json,
        saved_output=call.result,
        parsed_output=parsed,
        validation_errors=tuple(errors),
    )
