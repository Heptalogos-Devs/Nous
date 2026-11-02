"""Human rendering of the same compact business projection used by the SDK."""

from __future__ import annotations

import json

from pydantic import JsonValue
from rich.console import Console
from rich.table import Table
from rich.text import Text

from nous.platform.localization import Localizer

console = Console()


def present(value: JsonValue, locale: Localizer) -> None:
    if isinstance(value, dict):
        text = value.get("text")
        ref = value.get("ref")
        if isinstance(ref, str):
            label = value.get("title") or value.get("status") or value.get("state") or ""
            console.print(Text("[" + ref + "] " + str(label), style="bold"))
        if isinstance(text, str) and text:
            console.print(Text(text))
            if cursor := value.get("next_cursor"):
                console.print(Text("next_cursor: " + str(cursor), style="dim"))
            notices = value.get("notices")
            if isinstance(notices, list):
                for notice in notices:
                    console.print(Text(str(notice), style="yellow"))
            return
        items = value.get("items")
        if isinstance(items, list):
            if not items:
                console.print("No results found.")
                return
            if all(isinstance(item, dict) for item in items):
                columns = [
                    key
                    for key in ("ref", "slug", "name", "title", "kind", "status", "state")
                    if any(isinstance(item, dict) and key in item for item in items)
                ]
                if columns:
                    table = Table(show_header=True, header_style="bold")
                    for key in columns:
                        table.add_column(key, overflow="fold")
                    for item in items:
                        if isinstance(item, dict):
                            table.add_row(*(Text(str(item.get(key, ""))) for key in columns))
                    console.print(table)
                    return
        if isinstance(value.get("status"), str):
            console.print(Text(str(value["status"])))
            if reason := value.get("reason"):
                console.print(Text(str(reason)))
            return
    console.print(Text(json.dumps(value, ensure_ascii=False, indent=2)))
