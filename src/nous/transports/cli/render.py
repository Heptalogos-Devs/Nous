"""Readable summaries preserve references and actual body text."""

from __future__ import annotations

import json

from nous_sdk.contracts import QueryHit
from nous_sdk.presentation import object_text, reference_text
from pydantic import JsonValue
from rich.console import Console
from rich.table import Table
from rich.text import Text

from nous.platform.localization import Localizer

console = Console()


def _label(key: str, locale: Localizer) -> str:
    translated = "cli-" + key.replace("_", "-")
    return locale.text(translated) if locale.has(translated) else key.replace("_", " ")


def _cell(value: JsonValue, locale: Localizer) -> str:
    if value is None:
        return "—"
    if isinstance(value, bool):
        return locale.text("cli-yes" if value else "cli-no")
    if isinstance(value, (int, float)):
        return locale.number(value)
    if isinstance(value, str):
        return value
    if isinstance(value, dict) and set(value) == {"kind", "id", "revision"}:
        return f"{value['kind']}:{value['id']}@{value['revision']}"
    if isinstance(value, list) and value and all(isinstance(item, (int, float)) for item in value):
        return locale.text("cli-numbers", count=len(value))
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def _table(items: list[dict[str, JsonValue]], locale: Localizer) -> None:
    priority = (
        "name",
        "primary_name",
        "person_name",
        "title",
        "kind",
        "status",
        "state",
        "namespace_id",
        "subject_id",
        "material_id",
        "commit_seq",
    )
    identity = next(
        (
            key
            for key in ("episode_id", "person_id", "reference", "id")
            if any(key in item for item in items)
        ),
        None,
    )
    keys = ([identity] if identity else []) + [
        key for key in priority if any(key in item for item in items)
    ]
    if not keys:
        keys = list(dict.fromkeys(key for item in items for key in item))[:6]
    table = Table(show_lines=False, header_style="bold", expand=False)
    for key in keys:
        table.add_column(_label(key, locale), overflow="fold")
    for item in items:
        table.add_row(*(Text(_cell(item.get(key), locale)) for key in keys))
    console.print(table)
    console.print(locale.text("cli-items", count=len(items)), style="dim")


def _episode_record(value: JsonValue) -> dict[str, JsonValue] | None:
    if not isinstance(value, dict) or not isinstance(record := value.get("record"), dict):
        return None
    body = record.get("body")
    return record if isinstance(body, dict) and body.get("kind") == "episode" else None


def _episode(value: dict[str, JsonValue], locale: Localizer, *, expand: bool) -> None:
    record = _episode_record(value)
    assert record is not None
    body, addresses, reference = record["body"], record.get("addresses"), record.get("reference")
    assert isinstance(body, dict)
    exact = addresses.get("exact") if isinstance(addresses, dict) else None
    if not exact and isinstance(reference, dict):
        exact = f"episode:{reference['id']}@{reference['revision']}"
    console.print(Text(f"{exact}  {body.get('title', '')}", style="bold"))
    if summary := body.get("summary"):
        console.print(Text(str(summary)))
    members, sources = value.get("members"), value.get("sources")
    console.print(
        Text(
            f"{_label('members', locale)}: {len(members) if isinstance(members, list) else 0}"
            + f" · {_label('sources', locale)}: "
            + (", ".join(str(source) for source in sources) if isinstance(sources, list) else "—"),
            style="dim",
        )
    )
    if expand and isinstance(members, list):
        for member in members:
            if not isinstance(member, dict):
                continue
            address, content = member.get("addresses"), member.get("body")
            if isinstance(address, dict):
                console.print(Text(str(address.get("exact", "")), style="bold"))
            if isinstance(content, dict) and isinstance(text := content.get("text"), str):
                console.print(Text(text))


def present(value: JsonValue, locale: Localizer) -> None:
    if isinstance(value, dict) and _episode_record(value) is not None:
        _episode(value, locale, expand=True)
        return
    if isinstance(value, dict) and isinstance(value.get("items"), list):
        items = value["items"]
        if (
            isinstance(items, list)
            and items
            and all(
                isinstance(item, dict) and "value_schema" in item and "write_access" in item
                for item in items
            )
        ):
            _table(
                [
                    {"key": item["key"], "write_access": item["write_access"]}
                    for item in items
                    if isinstance(item, dict)
                ],
                locale,
            )
            return
        present(items, locale)
        for key, item in value.items():
            if key in {"items", "count"} or key == "success" and item is True:
                continue
            console.print(Text(f"{_label(key, locale)}: {_cell(item, locale)}"))
        return
    if isinstance(value, dict) and isinstance(value.get("hits"), list):
        hits = value["hits"]
        assert isinstance(hits, list)
        coverage = value.get("coverage")
        rerank = value.get("rerank")
        if isinstance(rerank, dict) and rerank.get("mode") != "off":
            console.print(
                Text(
                    f"{_label('rerank', locale)}: {rerank.get('status')} · {rerank.get('reason') or rerank.get('call_id') or ''}"
                )
            )
        if isinstance(coverage, dict):
            console.print(
                locale.text(
                    "query-summary",
                    count=len(hits),
                    complete=_cell(coverage.get("complete"), locale),
                    indexed=_cell(coverage.get("indexed_through"), locale),
                    authority=_cell(coverage.get("authority_commit_seq"), locale),
                )
            )
        for hit in hits:
            if not isinstance(hit, dict):
                continue
            obj = hit.get("object")
            if not isinstance(obj, dict):
                continue
            parsed = QueryHit.model_validate(hit)
            console.print(
                Text(
                    f"{reference_text(parsed.object)}  {_label('score', locale)} {_cell(hit.get('score'), locale)}",
                    style="bold",
                )
            )
            console.print(Text(object_text(parsed.object)))
        sources = value.get("sources")
        if isinstance(sources, dict):
            nodes = sources.get("nodes")
            if isinstance(nodes, list):
                _table([node for node in nodes if isinstance(node, dict)], locale)
            console.print(
                Text(f"{_label('complete', locale)}: {_cell(sources.get('complete'), locale)}")
            )
        return
    if isinstance(value, list):
        if not value:
            console.print(locale.text("cli-items", count=0))
            return
        records = [item for item in value if isinstance(item, dict)]
        if (
            records
            and len(records) == len(value)
            and all(_episode_record(item) for item in records)
        ):
            for item in records:
                _episode(item, locale, expand=False)
                console.print()
            console.print(locale.text("cli-items", count=len(records)), style="dim")
            return
        if len(records) == len(value) and records:
            _table(records, locale)
        else:
            console.print(Text(_cell(value, locale)))
        return
    if isinstance(value, dict):
        table = Table(show_header=False, box=None, padding=(0, 2))
        table.add_column(no_wrap=True)
        table.add_column(overflow="fold")
        for key, item in value.items():
            if key in ("catalog", "outputs", "file_successes") and isinstance(item, (dict, list)):
                table.add_row(_label(key, locale), locale.text("cli-items", count=len(item)))
            else:
                table.add_row(_label(key, locale), Text(_cell(item, locale)))
        console.print(table)
        return
    console.print(Text(_cell(value, locale)))
