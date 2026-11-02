"""Readonly Parquet and SQLite decoding into exact, source-backed native records."""

from __future__ import annotations

import ast
import csv
import json
import math
import sqlite3
from contextlib import closing
from itertools import chain
from pathlib import Path
from typing import Literal

import msgpack
import networkx as nx
import pyarrow.dataset as arrow_dataset
from nous_sdk.contracts import (
    ImportDocument,
    ImportRecord,
    ProposedAssociation,
    TimeRange,
)
from nous.platform.errors import DomainError
from nous_sdk.contracts.imports import ImportAssertions, ImportReference
from pydantic import ConfigDict, JsonValue, TypeAdapter, ValidationError

from nous.platform.persistence.authority import canonical_json
from nous.platform.time import parse_source_time

from .parsing import ParsedRecord
from .structured_records import assertions_from_source

_JSON = TypeAdapter(dict[str, JsonValue])
_PORTABLE = TypeAdapter(object, config=ConfigDict(ser_json_bytes="base64"))


def dataset_key(document: ImportDocument) -> str:
    return "dataset:" + (
        document.origin.get("dataset_ref")
        or document.file_identity
        or document.source
        or document.filename
    )


def triples(value: str) -> list[JsonValue]:
    for decoder in (json.loads, ast.literal_eval):
        try:
            parsed = _PORTABLE.validate_python(decoder(value))
        except ValueError, SyntaxError:
            continue
        if isinstance(parsed, (list, tuple)):
            parts = TypeAdapter(tuple[object, ...]).validate_python(parsed)
            if len(parts) == 3:
                return [str(item).strip() for item in parts]
    parsed = next(csv.reader([value.strip().removeprefix("(").removesuffix(")")]))
    if len(parsed) != 3:
        raise ValueError("Relation text must contain a complete three-part tuple")
    return [part.strip().strip("'\"") for part in parsed]


def string_list(value: JsonValue) -> list[str]:
    if value is None or value == "":
        return []
    if isinstance(value, str):
        try:
            value = _PORTABLE.dump_python(json.loads(value), mode="json")
        except ValueError:
            value = _PORTABLE.dump_python(ast.literal_eval(value), mode="json")
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise ValueError("Source list fields require arrays of actual strings")
    return list(
        dict.fromkeys(item.strip() for item in value if isinstance(item, str) and item.strip())
    )


def selected(record: ImportRecord, sequence: int, document: ImportDocument) -> bool:
    selection = document.selection
    if selection is None:
        return True
    if selection.after_sequence is not None and sequence <= selection.after_sequence:
        return False
    if selection.through_sequence is not None and sequence > selection.through_sequence:
        return False
    if (
        selection.conversation_refs is not None
        and record.origin.get("conversation_ref") not in selection.conversation_refs
    ):
        return False
    if selection.event:
        point = record.event_time
        if point is None or not selection.event.contains(point):
            return False
    return True


def parquet_records(path: Path, document: ImportDocument) -> list[ParsedRecord]:
    result: list[ParsedRecord] = []
    source = arrow_dataset.dataset(path, format="parquet")
    kind = document.source_type.removeprefix("lpmm_")
    if kind not in {"paragraph", "entity", "relation"}:
        kind = Path(document.filename).stem
    index = 0
    for batch in source.to_batches(batch_size=document.read_batch_size or 1024):
        for raw in batch.to_pylist():
            sequence = index + 1
            try:
                row = TypeAdapter(dict[str, object]).validate_python(raw)
                archived_vector = row.pop("embedding", None)
                embedding = (
                    TypeAdapter(tuple[float, ...]).validate_python(archived_vector)
                    if archived_vector is not None
                    else None
                )
                dimensions = len(embedding) if embedding is not None else None
                if embedding is not None and (
                    any(not math.isfinite(float(item)) for item in embedding)
                    or document.declared_vector_dimensions is not None
                    and dimensions != document.declared_vector_dimensions
                ):
                    raise ValueError("Archived source vector has invalid dimensions or coordinates")
                fields = _JSON.validate_python(_PORTABLE.dump_python(row, mode="json"))
                text = fields.get("str") or fields.get("content") or fields.get("text")
                identifiers: dict[str, str] = {}
                source_id = str(fields.get("hash") or f"row:{sequence}")
                data: dict[str, JsonValue] = {
                    "entities": fields.get("entities") or [],
                    "relations": fields.get("relations") or [],
                }
                if kind == "entity":
                    if not isinstance(text, str) or not text.strip():
                        raise ValueError("Entity Parquet requires a nonempty str/content column")
                    entity: dict[str, JsonValue] = {
                        "name": text,
                        "entity_type": fields.get("entity_type", "other"),
                        "aliases": fields.get("aliases", []),
                        "description": fields.get("description", ""),
                        "identifiers": fields.get("identifiers", {}),
                    }
                    if fields.get("hash"):
                        entity["identifiers"] = {
                            **TypeAdapter(dict[str, str]).validate_python(entity["identifiers"]),
                            dataset_key(document): source_id,
                        }
                    data["entities"] = [entity]
                elif kind == "relation":
                    relation = (
                        [fields.get("subject"), fields.get("predicate"), fields.get("object")]
                        if {"subject", "predicate", "object"}.issubset(fields)
                        else triples(str(text or ""))
                    )
                    data["relations"] = [
                        {
                            "subject": relation[0],
                            "predicate": relation[1],
                            "object": relation[2],
                            "external_ids": {dataset_key(document): source_id},
                        }
                    ]
                    text = text or canonical_json(
                        {key: fields.get(key) for key in ("subject", "predicate", "object")}
                    )
                elif kind == "paragraph" and fields.get("hash"):
                    identifiers[dataset_key(document)] = source_id
                if not isinstance(text, str) or not text.strip():
                    raise ValueError("Parquet row requires explicit textual content")
                record = ImportRecord(
                    external_id=source_id,
                    text=text,
                    source=document.source or document.filename,
                    source_type=document.source_type,
                    tags=TypeAdapter(tuple[str, ...]).validate_python(fields.get("tags") or ()),
                    identifiers=identifiers,
                    metadata=TypeAdapter(dict[str, JsonValue]).validate_python(
                        fields.get("metadata") or {}
                    ),
                    structured={
                        "format": "parquet",
                        "payload": fields,
                        "source_row": sequence,
                        "source_vector_dimensions": dimensions,
                    },
                    event_time=parse_source_time(fields.get("event_time"))
                    if fields.get("event_time") is not None
                    else None,
                    observed_at=parse_source_time(fields.get("observed_at"))
                    if fields.get("observed_at") is not None
                    else None,
                    origin=TypeAdapter(dict[str, str]).validate_python(fields.get("origin") or {}),
                    assertions=ImportAssertions.model_validate(fields["assertions"])
                    if fields.get("assertions") is not None
                    else assertions_from_source(data, identifier_scope=dataset_key(document)),
                )
                if "validity" in fields:
                    record = record.model_copy(
                        update={
                            "validity": TimeRange.model_validate(fields["validity"])
                            if fields["validity"] is not None
                            else None
                        }
                    )
                if selected(record, sequence, document):
                    result.append(
                        ParsedRecord(
                            index=index, record=record, extraction=f"pyarrow.parquet:row:{sequence}"
                        )
                    )
            except (ValueError, TypeError, ValidationError) as error:
                result.append(ParsedRecord(index=index, record=None, error=str(error)))
            index += 1
            if (
                document.selection
                and document.selection.limit is not None
                and len(result) >= document.selection.limit
            ):
                return result
    return result


def sqlite_records(path: Path, document: ImportDocument) -> list[ParsedRecord]:
    result: list[ParsedRecord] = []
    with closing(
        sqlite3.connect(path.resolve().as_uri() + "?mode=ro&immutable=1", uri=True)
    ) as connection:
        connection.row_factory = sqlite3.Row
        tables = {
            str(row[0])
            for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
        if document.source_type == "maibot_history":
            table = (
                "chat_history"
                if "chat_history" in tables
                else "mai_messages"
                if "mai_messages" in tables
                else None
            )
        elif document.source_type == "legacy_metadata":
            table = "paragraphs" if "paragraphs" in tables else None
        else:
            table = (
                "chat_history"
                if "chat_history" in tables
                else "paragraphs"
                if "paragraphs" in tables
                else None
            )
            if table is None and "mai_messages" in tables:
                table = "mai_messages"
        if table is None:
            raise DomainError(
                "source_schema_unknown", "SQLite source requires chat_history or paragraphs"
            )
        columns = {str(row[1]) for row in connection.execute(f'PRAGMA table_info("{table}")')}
        if table == "mai_messages" and not {
            "id",
            "message_id",
            "timestamp",
            "session_id",
            "raw_content",
        }.issubset(columns):
            raise DomainError(
                "source_schema_unknown",
                "mai_messages does not contain the actual stored message schema",
            )
        if table == "chat_history" and not (
            {"chat_id", "start_time", "end_time"}.issubset(columns)
            or {"session_id", "start_timestamp", "end_timestamp"}.issubset(columns)
        ):
            raise DomainError(
                "source_schema_unknown", "chat_history does not have a supported actual schema"
            )
        selection = document.selection
        if selection and (selection.group_refs or selection.user_refs):
            conversations = set(selection.conversation_refs or ())
            stream = next(
                (
                    (name, identifier)
                    for name, identifier in (
                        ("chat_streams", "stream_id"),
                        ("chat_sessions", "session_id"),
                    )
                    if name in tables
                    and {identifier, "group_id", "user_id"}.issubset(
                        {str(row[1]) for row in connection.execute(f'PRAGMA table_info("{name}")')}
                    )
                ),
                None,
            )
            if stream is None:
                raise DomainError(
                    "source_stream_schema_missing",
                    "Source does not contain the actual group/user conversation mapping",
                )
            for column, values in (
                ("group_id", selection.group_refs),
                ("user_id", selection.user_refs),
            ):
                if values:
                    query = (
                        f'SELECT "{stream[1]}" FROM "{stream[0]}" WHERE "{column}" IN ('
                        + ",".join("?" for _ in values)
                        + ")"
                    )
                    conversations.update(str(row[0]) for row in connection.execute(query, values))
            document = document.model_copy(
                update={
                    "selection": selection.model_copy(
                        update={"conversation_refs": tuple(sorted(conversations))}
                    )
                }
            )
        cursor = connection.execute(
            f'SELECT * FROM "{table}" ORDER BY ' + ("id" if "id" in columns else "rowid")
        )
        for index, raw in enumerate(
            chain.from_iterable(
                iter(lambda: cursor.fetchmany(document.read_batch_size or 1024), [])
            )
        ):
            try:
                row = _JSON.validate_python(_PORTABLE.dump_python(dict(raw), mode="json"))
                sequence = int(str(row.get("id") or index + 1))
                if table == "chat_history":
                    chat = str(row.get("chat_id") or row.get("session_id") or "")
                    participants, keywords = (
                        string_list(row.get("participants")),
                        string_list(row.get("keywords")),
                    )
                    theme, summary = (
                        str(row.get("theme") or "").strip(),
                        str(row.get("summary") or "").strip(),
                    )
                    if not theme and not summary:
                        raise ValueError("Chat history row has no actual theme or summary")
                    text = f"话题：{theme}\n概括：{summary}\n参与者：{'、'.join(participants)}\n关键词：{'、'.join(keywords)}"
                    graph = _JSON.validate_python(
                        {
                            "entities": [
                                {"name": name, "entity_type": "person"} for name in participants
                            ],
                            "relations": row.get("relations") or [],
                        }
                    )
                    event = parse_source_time(row.get("start_time", row.get("start_timestamp")))
                    record = ImportRecord(
                        external_id=f"maibot:chat_history:{sequence}",
                        text=text,
                        source=f"maibot.chat_history:{chat}",
                        source_type="chat_summary",
                        event_time=event,
                        origin={"conversation_ref": chat},
                        metadata=_JSON.validate_python(
                            {
                                "source_row": sequence,
                                "source_event_end": row.get("end_time", row.get("end_timestamp")),
                                "participants": participants,
                                "keywords": keywords,
                            }
                        ),
                        structured={"format": "maibot_chat_history", "payload": row},
                        assertions=assertions_from_source(graph),
                    )
                elif table == "mai_messages":
                    packed = raw["raw_content"]
                    components = (
                        TypeAdapter(list[dict[str, JsonValue]]).validate_python(
                            msgpack.unpackb(packed, raw=False)
                        )
                        if packed
                        else []
                    )
                    texts = [
                        item["data"]
                        for item in components
                        if item.get("type") == "text" and isinstance(item.get("data"), str)
                    ]
                    text = (
                        "\n".join(str(item) for item in texts)
                        if texts
                        else canonical_json(components)
                        if components
                        else str(row.get("processed_plain_text") or "")
                    )
                    if not text.strip():
                        raise ValueError("Stored message has no actual content")
                    record = ImportRecord(
                        external_id="maibot:message:" + str(row["message_id"]),
                        text=text,
                        source="maibot.messages:" + str(row["session_id"]),
                        source_type="message",
                        event_time=parse_source_time(row.get("timestamp")),
                        observed_at=parse_source_time(row.get("received_at")),
                        origin={
                            "conversation_ref": str(row["session_id"]),
                            "platform": str(row.get("platform") or ""),
                            "user_ref": str(row.get("user_id") or ""),
                            "group_ref": str(row.get("group_id") or ""),
                        },
                        metadata={
                            "source_row": sequence,
                            "message_id": row["message_id"],
                            "text_projection": "raw_text_components"
                            if texts
                            else "stored_component_metadata",
                        },
                        structured=_JSON.validate_python(
                            {
                                "format": "maibot_message",
                                "payload": row,
                                "components": components,
                            }
                        ),
                    )
                else:
                    text = row.get("content")
                    if not isinstance(text, str) or not text.strip():
                        raise ValueError("Paragraph metadata requires actual content")
                    event = parse_source_time(
                        row.get("event_time")
                        if row.get("event_time") is not None
                        else row.get("event_time_start")
                    )
                    observed = (
                        parse_source_time(row.get("created_at"))
                        if document.origin.get("no_created_fallback") != "true"
                        else None
                    )
                    record = ImportRecord(
                        external_id=str(row.get("hash") or f"paragraph:{sequence}"),
                        text=text,
                        source=str(row.get("source") or document.filename),
                        source_type="legacy_paragraph",
                        event_time=event,
                        observed_at=observed,
                        metadata={
                            "source_row": sequence,
                            "legacy_created_at": row.get("created_at"),
                            "source_event_end": row.get("event_time_end"),
                        },
                        structured={"format": "legacy_metadata", "payload": row},
                    )
                if selected(record, sequence, document):
                    result.append(
                        ParsedRecord(
                            index=index, record=record, extraction=f"sqlite:{table}:row:{sequence}"
                        )
                    )
            except (ValueError, TypeError, ValidationError) as error:
                result.append(ParsedRecord(index=index, record=None, error=str(error)))
            if (
                document.selection
                and document.selection.limit is not None
                and len(result) >= document.selection.limit
            ):
                break
    return result


def graphml_records(path: Path, document: ImportDocument) -> list[ParsedRecord]:
    graph = nx.read_graphml(path)
    result: list[ParsedRecord] = []
    for index, (source, target, raw_attributes) in enumerate(graph.edges(data=True)):
        attributes = _JSON.validate_python(raw_attributes)
        source_id = (
            str(attributes["id"])
            if isinstance(attributes.get("id"), (str, int))
            else f"graph-edge:{index}"
        )
        bindings: list[ImportReference] = []
        for key, raw in (
            ("from", str(graph.nodes[source].get("name") or source)),
            ("to", str(graph.nodes[target].get("name") or target)),
        ):
            kind, _, identifier = raw.partition("-")
            native = {
                "paragraph": "memory",
                "chunk": "memory",
                "entity": "entity",
                "relation": "association",
                "fact": "association",
            }.get(kind)
            if native is None or not identifier:
                raise DomainError(
                    "source_graph_reference_unknown",
                    "GraphML node must identify a declared paragraph, entity or relation",
                    node=raw,
                )
            bindings.append(
                ImportReference.model_validate(
                    {
                        "key": key,
                        "kind": native,
                        "identifier_key": dataset_key(document),
                        "identifier": raw,
                        "aliases": (identifier,),
                    }
                )
            )
        payload = _JSON.validate_python(
            _PORTABLE.dump_python(
                {"from": source, "to": target, "attributes": attributes}, mode="json"
            )
        )
        record = ImportRecord(
            external_id=source_id,
            text=canonical_json(payload),
            source=document.source or document.filename,
            source_type="source_graph",
            structured={"format": "graphml", "payload": payload},
            assertions=ImportAssertions(
                associations=(
                    ProposedAssociation(
                        from_key="from",
                        to_key="to",
                        relation=str(
                            attributes.get("relation")
                            or attributes.get("predicate")
                            or "source_graph_edge"
                        ),
                        description=TypeAdapter(str).validate_python(
                            attributes.get("description", "")
                        ),
                        polarity=TypeAdapter(Literal["positive", "negative"]).validate_python(
                            attributes.get("polarity", "positive")
                        ),
                        external_ids={dataset_key(document): source_id},
                    ),
                ),
                references=tuple(bindings),
            ),
        )
        if selected(record, index + 1, document):
            result.append(
                ParsedRecord(
                    index=index, record=record, extraction=f"networkx.graphml:edge:{index}"
                )
            )
        if (
            document.selection
            and document.selection.limit is not None
            and len(result) >= document.selection.limit
        ):
            break
    return result
