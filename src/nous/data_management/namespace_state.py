"""Read one Namespace's Authority and portable scheduler facts from a SQLite snapshot."""

import sqlite3
from contextlib import closing
from pathlib import Path

from nous.platform.errors import DomainError
from pydantic import JsonValue, TypeAdapter

from nous.platform.persistence import metadata

type RawRow = dict[str, str | int | float | None]
_ROW = TypeAdapter(RawRow)
_OBJECT = TypeAdapter(dict[str, JsonValue])


def quoted(name: str) -> str:
    return '"' + name.replace('"', '""') + '"'


def capture(database: Path, namespace_id: str) -> tuple[int, dict[str, tuple[RawRow, ...]], tuple[str, ...]]:
    with closing(sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)) as connection:
        connection.row_factory = sqlite3.Row
        tables = {
            row[0]: {
                column[1] for column in connection.execute(f"PRAGMA table_info({quoted(row[0])})")
            }
            for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
        rows: dict[str, list[RawRow]] = {}
        for table, columns in tables.items():
            if (
                table in metadata.tables
                and "namespace_id" in columns
                and table != "nous_job_results"
            ):
                rows[table] = [
                    _ROW.validate_python(dict(row))
                    for row in connection.execute(
                        f"SELECT * FROM {quoted(table)} WHERE namespace_id=?", (namespace_id,)
                    )
                ]
        rows["namespaces"] = [
            _ROW.validate_python(dict(row))
            for row in connection.execute("SELECT * FROM namespaces WHERE id=?", (namespace_id,))
        ]
        if len(rows["namespaces"]) != 1:
            raise DomainError("namespace_not_found", "Namespace does not exist in the snapshot")

        def child(table: str, column: str, parent: str, parent_column: str = "id") -> None:
            if table not in tables:
                return
            values = {row[parent_column] for row in rows.get(parent, [])}
            selected = [
                _ROW.validate_python(dict(row))
                for row in connection.execute(f"SELECT * FROM {quoted(table)}")
                if row[column] in values
            ]
            if "namespace_id" in tables[table]:
                selected = list(
                    {
                        str(row.get("key", row.get("id"))): row
                        for row in (*rows.get(table, []), *selected)
                    }.values()
                )
            rows[table] = selected

        child("context_feedback", "context_id", "cognitive_contexts")
        child("import_inputs", "catalog_id", "import_catalogs")
        child("import_units", "catalog_id", "import_catalogs")
        child("import_outputs", "unit_id", "import_units")
        child("import_manifest_heads", "success_id", "import_file_successes")
        child("vector_projection_claims", "request_id", "vector_projection_requests")
        child("vector_projection_subscriptions", "request_id", "vector_projection_requests")
        rows.pop("namespace_backup_uploads", None)
        for table in ("import_requests",):
            rows[table] = []
            for row in connection.execute(f"SELECT * FROM {quoted(table)}"):
                value = _OBJECT.validate_json(str(row["record"]))
                context_raw = value.get("context", {})
                context = context_raw if isinstance(context_raw, dict) else {}
                if (
                    context.get("namespace_id") == namespace_id
                    or value.get("namespace_id") == namespace_id
                ):
                    rows[table].append(_ROW.validate_python(dict(row)))
        workflow_ids = tuple(str(row['workflow_id']) for row in rows.pop('nous_job_links', []))
        commit = connection.execute(
            "SELECT commit_seq FROM nous_commit_clock WHERE singleton=1"
        ).fetchone()[0]
        return int(commit), {name: tuple(values) for name, values in rows.items()}, workflow_ids
