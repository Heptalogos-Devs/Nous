"""Ordered JSON/JSONL decoding adopted from the existing import_format owner.

The original contract and normalization are in crates/nous-app/src/import_format.rs.
JSON parsing belongs to the standard library; this module owns record semantics.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from hashlib import sha256
from typing import Literal

from nous_sdk.contracts import DomainError, ImportRecord
from pydantic import JsonValue, TypeAdapter, ValidationError

from nous.platform.time import parse_source_time, parse_timepoint


@dataclass(frozen=True)
class ParsedRecord:
    index: int
    record: ImportRecord | None
    pointer: str | None = None
    line_number: int | None = None
    error: str | None = None
    external_id: str | None = None
    page_number: int | None = None
    extraction: str | None = None
    media_required: bool = False


def stable_id(value: object) -> str:
    return (
        "sha256:"
        + sha256(
            json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
        ).hexdigest()
    )


def _record(
    value: JsonValue,
    index: int,
    kind: str,
    pointer: str | None,
    line_number: int | None = None,
    identifier_scope: str | None = None,
) -> ParsedRecord:
    try:
        if kind == "text":
            record = ImportRecord(external_id=stable_id({"text": value}), text=str(value))
        elif kind == "native":
            if not isinstance(value, dict):
                raise ValueError("Native import records must be objects")
            # JSON loads has already produced portable values. Pydantic owns field validation.
            fields = dict(value)
            fields.setdefault("external_id", stable_id(value))
            if fields.get("origin") is None:
                fields["origin"] = {}
            for axis in ("event_time", "observed_at"):
                expression = fields.get(axis)
                if isinstance(expression, str):
                    point = parse_timepoint(expression)
                    fields[axis] = point.model_dump(mode="json") if point else None
            record = ImportRecord.model_validate(fields)
        else:
            field = "passage" if kind == "lpmm_openie" else "content"
            if kind == "a_memorix_paragraph" and isinstance(value, str):
                text = value
                fields = {}
            elif isinstance(value, dict):
                text = value.get(field)
                fields = value
            else:
                raise ValueError("Paragraph content or LPMM passage must be a string")
            if not isinstance(text, str):
                raise ValueError("Paragraph content or LPMM passage must be a string")
            structured = {"format": kind, "payload": value}
            from .structured_records import assertions_from_source

            assertions = assertions_from_source(
                fields, openie=kind == "lpmm_openie", identifier_scope=identifier_scope
            )
            supplied_id = next(
                (
                    fields[key]
                    for key in ("external_id", "idx", "hash")
                    if fields.get(key) is not None
                ),
                None,
            )
            record = ImportRecord.model_validate(
                {
                    "external_id": str(supplied_id)
                    if supplied_id is not None
                    else stable_id(structured),
                    "identifiers": {identifier_scope: str(supplied_id)}
                    if identifier_scope and supplied_id is not None
                    else {},
                    "text": text,
                    "source": fields.get("source"),
                    "source_type": fields.get("source_type") or kind,
                    "tags": fields.get("tags", []),
                    "metadata": fields.get("metadata", {}),
                    "structured": structured,
                    "assertions": assertions,
                    **(
                        {
                            "event_time": parse_source_time(
                                fields.get("event_time")
                                if fields.get("event_time") is not None
                                else fields.get("event_time_start")
                            )
                        }
                        if "event_time" in fields or "event_time_start" in fields
                        else {}
                    ),
                    **(
                        {"observed_at": parse_source_time(fields["observed_at"])}
                        if "observed_at" in fields
                        else {}
                    ),
                }
            )
        if not record.text.strip():
            raise ValueError("Import text is empty")
        return ParsedRecord(
            index=index,
            record=record,
            pointer=pointer,
            line_number=line_number,
            external_id=record.external_id,
        )
    except (ValueError, ValidationError) as error:
        return ParsedRecord(
            index=index,
            record=None,
            pointer=pointer,
            line_number=line_number,
            error=str(error),
            external_id=None,
        )


def parse_document(
    content: str, format: Literal["text", "json", "jsonl"], *, identifier_scope: str | None = None
) -> list[ParsedRecord]:
    if not content.strip():
        raise DomainError("empty_import", "Import content is empty")
    if format == "text":
        return [_record(content, 0, "text", None)]
    if format == "jsonl":
        result: list[ParsedRecord] = []
        for number, line in enumerate(content.splitlines(), 1):
            if not line.strip():
                continue
            try:
                value = json.loads(line)
                result.append(_record(value, len(result), "native", None, line_number=number))
            except json.JSONDecodeError as error:
                result.append(
                    ParsedRecord(
                        index=len(result),
                        record=None,
                        line_number=number,
                        error=f"JSONL line {number}: {error.msg}",
                    )
                )
        return result
    try:
        payload = TypeAdapter(JsonValue).validate_python(json.loads(content))
    except json.JSONDecodeError as error:
        raise DomainError(
            "invalid_import_json",
            f"Invalid JSON at line {error.lineno}, column {error.colno}: {error.msg}",
        ) from error
    context: JsonValue = None
    if isinstance(payload, list):
        records, kind, prefix = payload, "native", ""
    elif isinstance(payload, dict):
        if payload.get("format") == "a-memorix-memory-bundle":
            raise DomainError("bundle_required", "Memory bundles require the bundle installer")
        keys = [key for key in ("records", "paragraphs", "docs") if key in payload]
        if len(keys) != 1 or not isinstance(payload[keys[0]], list):
            raise DomainError(
                "invalid_import_json", "JSON requires one records, paragraphs, or docs array"
            )
        key = keys[0]
        records_value = payload[key]
        assert isinstance(records_value, list)
        records = records_value
        kind = {"records": "native", "paragraphs": "a_memorix_paragraph", "docs": "lpmm_openie"}[
            key
        ]
        context = {name: value for name, value in payload.items() if name != key}
        prefix = "/" + key
    else:
        raise DomainError(
            "invalid_import_json",
            "JSON must contain a record array or records/paragraphs/docs envelope",
        )
    result = []
    for index, value in enumerate(records):
        pointer = f"{prefix}/{index}"
        result.append(_record(value, index, kind, pointer, identifier_scope=identifier_scope))
    if isinstance(context, dict):
        from .structured_records import assertions_from_source

        for field in ("entities", "relations"):
            values = context.get(field) or []
            if not isinstance(values, list):
                raise DomainError("invalid_import_json", "Global source assertions require arrays")
            for index, value in enumerate(values):
                try:
                    asserted = assertions_from_source(
                        {field: [value]}, identifier_scope=identifier_scope
                    )
                    structured = {"format": "source_assertion", "payload": value}
                    record = ImportRecord(
                        external_id=stable_id(structured),
                        text=json.dumps(value, ensure_ascii=False, sort_keys=True),
                        source_type="source_assertion",
                        structured=structured,
                        assertions=asserted,
                    )
                    result.append(
                        ParsedRecord(index=len(result), record=record, pointer=f"/{field}/{index}")
                    )
                except (ValueError, ValidationError) as error:
                    result.append(
                        ParsedRecord(
                            index=len(result),
                            record=None,
                            pointer=f"/{field}/{index}",
                            error=str(error),
                        )
                    )
    return result
