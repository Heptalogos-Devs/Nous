"""Readable summaries preserve references and actual body text."""

from __future__ import annotations

import json

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
    if isinstance(value, list) and value and all(isinstance(item, (int, float)) for item in value):
        return locale.text("cli-numbers", count=len(value))
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def _table(items: list[dict[str, JsonValue]], locale: Localizer) -> None:
    priority = (
        "id",
        "reference",
        "name",
        "kind",
        "status",
        "state",
        "namespace_id",
        "subject_id",
        "material_id",
        "commit_seq",
    )
    keys = [key for key in priority if any(key in item for item in items)]
    if not keys:
        keys = list(dict.fromkeys(key for item in items for key in item))[:6]
    table = Table(show_lines=False, header_style="bold", expand=False)
    for key in keys:
        table.add_column(_label(key, locale), overflow="fold")
    for item in items:
        table.add_row(*(Text(_cell(item.get(key), locale)) for key in keys))
    console.print(table)
    console.print(locale.text("cli-items", count=len(items)), style="dim")


def present(value: JsonValue, locale: Localizer) -> None:
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
            addresses = obj.get("addresses")
            reference = (
                addresses.get("exact") if isinstance(addresses, dict) else obj.get("reference")
            )
            console.print(
                Text(
                    f"{_cell(reference, locale)}  {_label('score', locale)} {_cell(hit.get('score'), locale)}",
                    style="bold",
                )
            )
            body = obj.get("body")
            console.print(
                Text(
                    str(body.get("text", _cell(body, locale)))
                    if isinstance(body, dict)
                    else _cell(body, locale)
                )
            )
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
