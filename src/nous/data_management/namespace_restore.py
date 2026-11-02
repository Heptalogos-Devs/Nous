"""Restore one Namespace snapshot into its new Namespace identity."""

import hashlib
import json
from uuid import NAMESPACE_URL, uuid5

import sqlalchemy as sa
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.execution import ExecutionSnapshot
from nous_sdk.contracts.identity import Namespace, SubjectBinding
from nous_sdk.contracts.namespace_backups import (
    NamespaceBackupRestored,
    NamespaceState,
    RestoreNamespaceBackup,
)
from nous_sdk.contracts.references import SemanticTarget
from pydantic import JsonValue, TypeAdapter

from nous.identity.service import IdentityService
from nous.platform.content import ContentLease, ContentStore
from nous.platform.execution import DurableExecution
from nous.platform.persistence import Authority, metadata
from nous.platform.persistence.authority import canonical_json, operation_key
from nous.platform.persistence.schema import clock
from nous.references.allocation import allocate_target_in

from .namespace_state import quoted

_OBJECT = TypeAdapter(dict[str, JsonValue])
_TARGET = TypeAdapter(SemanticTarget)
_CHILDREN = {
    "context_feedback",
    "import_inputs",
    "import_units",
    "import_outputs",
    "import_manifest_heads",
    "import_requests",
    "vector_projection_claims",
    "vector_projection_subscriptions",
}
_JSON = {
    "record",
    "context",
    "value",
    "target",
    "request",
    "result",
    "usage",
    "vector",
    "source_references",
    "payload_json",
    "result_json",
    "jobs_json",
    "attributes",
    "inputs",
    "output",
    "error",
    "message",
}
_IDENTITIES = {
    "id",
    "assessment_id",
    "motivation_assessment_id",
    'namespace_id',
    "task_id",
    "job_id",
    "ranking",
    "material_id",
    "region_id",
    "representation_id",
    "observation_id",
    "context_id",
    "track_id",
    "session_id",
    "preparation_id",
    "admission_id",
    "request_id",
    "catalog_id",
    "unit_id",
    "output_id",
    "input_source_id",
    "success_id",
    "parent_catalog_id",
    "contact_observation_id",
    "previous_observation_id",
    "retry_parent_job_id",
    "workflow_id",
    "workflow_uuid",
    "destination_uuid",
    "message_uuid",
    "owned_parent",
    "child_workflow_id",
    "parent_workflow_id",
    "forked_from",
    "installation_id",
    "bundle_id",
    "content_key",
    "key",
    "operation_key",
    "schedule_id",
    "schedule_name",
    "scope",
    "owner_ref",
    "address",
    "object",
    "exact",
    "reference",
    "references",
    "source_ids",
    "source_roots",
    "support_roots",
    "material_ids",
    "input_source_ids",
    "required_source_ids",
    "required_observation_ids",
    "credential_source_ids",
    "allowed_source_ids",
    "accepted_observation_ids",
    "source_material_ids",
    "refs",
    "unit_outputs",
}
_LITERAL = {
    "origin",
    "identifiers",
    "metadata",
    "structured",
    "external_ids",
    "provenance",
    "source_origin",
    "request_hash",
    "text",
    "context_text",
    "rendered",
    "filename",
    "source",
    "external_event_id",
    "external_id",
}


def document(value: JsonValue) -> JsonValue:
    return json.loads(value) if isinstance(value, str) else value


def validate(state: NamespaceState) -> None:
    allowed = (
        {
            name
            for name, table in metadata.tables.items()
            if "namespace_id" in table.c and name != "namespace_backup_uploads"
        }
        | _CHILDREN
        | {"namespaces"}
    )
    if set(state.tables) - allowed:
        raise DomainError(
            "namespace_backup_integrity",
            "Backup includes data outside Namespace ownership",
            tables=sorted(set(state.tables) - allowed),
        )
    for name, rows in state.tables.items():
        for row in rows:
            if (
                "namespace_id" in row
                and row["namespace_id"] != state.namespace_id
            ):
                raise DomainError(
                    "namespace_backup_scope_forbidden",
                    "Backup row belongs to another Namespace",
                    table=name,
                )
    if (
        len(state.tables.get("namespaces", ())) != 1
        or state.tables["namespaces"][0]["id"] != state.namespace_id
    ):
        raise DomainError(
            "namespace_backup_integrity", "Backup has no single consistent Namespace identity"
        )
    def trusted_contexts(value: JsonValue) -> None:
        if isinstance(value, dict):
            if {"caller_id", "namespace_id", "credential_source_ids"}.issubset(value):
                if value["namespace_id"] != state.namespace_id:
                    raise DomainError(
                        "namespace_backup_scope_forbidden",
                        "Saved work contains a context outside the archived Namespace",
                    )
            for field, item in value.items():
                if field not in _LITERAL:
                    trusted_contexts(item)
        elif isinstance(value, list):
            for item in value:
                trusted_contexts(item)

    for rows in state.tables.values():
        for row in rows:
            if row.get("namespace_id") is not None and str(row["namespace_id"]) != state.namespace_id:
                raise DomainError(
                    "namespace_backup_scope_forbidden",
                    "Backup row belongs to another Namespace",
                )
            for field in ("context", "inputs"):
                if row.get(field) is not None:
                    trusted_contexts(document(row[field]))
            record = row.get("record")
            if record is not None:
                decoded = document(record)
                if isinstance(decoded, dict) and "context" in decoded:
                    trusted_contexts(decoded["context"])
    for child, field, parent, parent_field in (
        ("context_feedback", "context_id", "cognitive_contexts", "id"),
        ("import_inputs", "catalog_id", "import_catalogs", "id"),
        ("import_units", "catalog_id", "import_catalogs", "id"),
        ("import_outputs", "unit_id", "import_units", "id"),
        ("import_manifest_heads", "success_id", "import_file_successes", "id"),
        ("vector_projection_claims", "request_id", "vector_projection_requests", "id"),
        ("vector_projection_subscriptions", "request_id", "vector_projection_requests", "id"),
    ):
        identities = {str(row[parent_field]) for row in state.tables.get(parent, ())}
        if any(str(row[field]) not in identities for row in state.tables.get(child, ())):
            raise DomainError(
                "namespace_backup_integrity", "Backup has an unresolved owned row", table=child
            )


async def restore(
    authority: Authority,
    identity_owner: IdentityService,
    content: ContentStore,
    execution: DurableExecution,
    ctx: CallContext,
    command: RestoreNamespaceBackup,
    state: NamespaceState,
    leases: dict[str, ContentLease],
) -> NamespaceBackupRestored:
    validate(state)
    if command.preserve_ids and ctx.namespace_id != state.namespace_id:
        raise DomainError(
            "namespace_backup_identity",
            "Identity-preserving restore requires the original Namespace identity",
        )
    if ctx.namespace_id == "instance":
        raise DomainError("namespace_backup_scope_forbidden", "Select a new receiving Namespace")
    async with authority.write(ctx) as tx:
        original = state.tables
        mapping: dict[str, str] = {state.namespace_id: ctx.namespace_id}

        def identity(value: str) -> str:
            return (
                value
                if command.preserve_ids
                else str(
                    uuid5(
                        NAMESPACE_URL,
                        f"nous:namespace-copy:{ctx.namespace_id}:{command.backup_id}:{value}",
                    )
                )
            )

        for name, rows in original.items():
            for row in rows:
                for field in (
                    "id",
                    "workflow_uuid",
                    "message_uuid",
                    "schedule_id",
                    "schedule_name",
                ):
                    value = row.get(field)
                    if isinstance(value, str) and (field != "id" or name != "namespaces"):
                        mapping.setdefault(value, identity(value))
                if name in {
                    "vector_projection_contents",
                    "vector_projection_claims",
                    "import_manifest_heads",
                } and isinstance(key := row.get("key"), str):
                    mapping.setdefault(key, identity(key))
        for job in state.execution.jobs:
            mapping.setdefault(job.workflow_id, identity(job.workflow_id))
        old_sequences = sorted(
            {
                TypeAdapter(int).validate_python(row["commit_seq"])
                for rows in original.values()
                for row in rows
                if isinstance(row.get("commit_seq"), int)
            }
        )
        sequences = {old: tx.commit_seq + index + 1 for index, old in enumerate(old_sequences)}
        final_seq = tx.commit_seq + len(sequences)

        def token(value: str) -> str:
            if value in mapping:
                return mapping[value]
            if ":" in value:
                kind, rest = value.split(":", 1)
                identifier, at, revision = rest.partition("@")
                if identifier in mapping:
                    return kind + ":" + mapping[identifier] + ("@" + revision if at else "")
                nested = token(rest)
                if nested != rest:
                    return kind + ":" + nested
            return value

        def remap(value: JsonValue, field: str = "", *, arguments: bool = False) -> JsonValue:
            if (
                field == "source"
                and isinstance(value, dict)
                and value.get("kind")
                in {"material", "source_region", "representation", "derived_region"}
                and isinstance(value.get("id"), str)
            ):
                return {key: remap(item, key) for key, item in value.items()}
            if field == "provenance" and isinstance(value, dict):
                return {
                    key: remap(item, key)
                    if key in {"motivation_assessment_id", "motivation_assessment_commit_seq"}
                    else item
                    for key, item in value.items()
                }
            if field in _LITERAL:
                return value
            if isinstance(value, dict):
                if field == "relevance":
                    return {token(key): item for key, item in value.items()}
                if field == "unit_outputs":
                    return {token(key): remap(item, "output_id") for key, item in value.items()}
                if field == "source_regions":
                    return {token(key): remap(item, "reference") for key, item in value.items()}
                if (
                    not command.preserve_ids
                    and "caller_id" in value
                    and "namespace_id" in value
                    and "credential_source_ids" in value
                ):
                    value = {
                        **value,
                        "caller_id": ctx.caller_id,
                        "credential_source_ids": list(ctx.credential_source_ids)
                        if ctx.credential_source_ids is not None
                        else None,
                    }
                return {
                    key: remap(
                        item,
                        key,
                        arguments=arguments or key in {"args", "positionalArgs", "namedArgs"},
                    )
                    for key, item in value.items()
                }
            if isinstance(value, list):
                return [remap(item, field, arguments=arguments) for item in value]
            if isinstance(value, int) and (
                field == "commit_seq"
                or field.endswith("_commit_seq")
                or field in {"as_of_commit", "authority_commit_seq", "admitted_commit"}
            ):
                return sequences.get(value, value)
            if isinstance(value, str) and (
                field in _IDENTITIES or field.endswith("_ids") or arguments
            ):
                return token(value)
            return value

        # Namespace identities are accepted by the lifecycle owner in this same transaction.
        ns_row = dict(original['namespaces'][0])
        ns_row['enabled_features'] = document(ns_row['enabled_features'])
        ns = Namespace.model_validate(remap(ns_row))
        if not command.preserve_ids:
            ns = ns.model_copy(update={'slug':ns.slug+'-'+ctx.namespace_id[:8], 'subject_key':ctx.namespace_id if ns.subject_key is not None else None})
        restored_bindings = tuple(
            SubjectBinding.model_validate(remap(dict(row)))
            for row in original.get("subject_bindings", ())
        )
        await identity_owner.restore_directory_in(tx, ns, restored_bindings)

        # Reallocate readable names only for a new identity. Exact installation restore retains them.
        if not command.preserve_ids:
            for row in original.get("reference_addresses", ()):
                target = _TARGET.validate_python(remap(document(row["target"])))
                mapping[str(row["address"])] = await allocate_target_in(tx, ctx, target)
        paused: list[str] = []
        skip = (
            {
                "namespaces",
                "subject_bindings",
                "nous_content_references",
                "reference_addresses",
            }
            if not command.preserve_ids
            else {"namespaces", "subject_bindings", "nous_content_references"}
        )
        skip |= {
            "nous_job_links",
            "nous_job_results",
        }
        tables = sorted(
            original,
            key=lambda name: (
                0 if name == "nous_operations" else 1,
                name,
            ),
        )
        for name in tables:
            if name in skip:
                continue
            columns = {
                str(item[1])
                for item in (
                    await tx.connection.exec_driver_sql(f"PRAGMA table_info({quoted(name)})")
                ).all()
            }
            if not columns:
                raise DomainError(
                    "namespace_backup_migration_required",
                    "Backup requires an unavailable table",
                    table=name,
                )
            for raw in original[name]:
                if set(raw) - columns:
                    raise DomainError(
                        "namespace_backup_migration_required",
                        "Backup table requires unavailable fields",
                        table=name,
                    )
                values: dict[str, JsonValue] = {}
                for field, value in raw.items():
                    if field in _JSON and isinstance(value, str):
                        try:
                            decoded = TypeAdapter(JsonValue).validate_json(value)
                            changed = remap(decoded, field)
                            values[field] = canonical_json(changed)
                        except (ValueError, TypeError):
                            if field in {"output", "error", "message"}:
                                values[field] = value
                            else:
                                raise DomainError(
                                    "namespace_backup_integrity",
                                    "Backup contains invalid stored JSON",
                                    table=name,
                                    field=field,
                                ) from None
                    else:
                        values[field] = remap(value, field)
                if name == "nous_operations":
                    operation_ctx = ctx.model_copy(
                        update={
                            "caller_id": str(values["caller_id"]),
                            
                        }
                    )
                    old_key = str(raw["operation_key"])
                    values["operation_key"] = operation_key(
                        operation_ctx,
                        str(values["operation_kind"]),
                        str(values["scope"]),
                        str(values["operation_id"]),
                    )
                    mapping[old_key] = str(values["operation_key"])
                    if values.get("payload_json") is not None:
                        values["payload_hash"] = hashlib.sha256(
                            canonical_json(
                                {
                                    "kind": values["operation_kind"],
                                    "payload": json.loads(str(values["payload_json"])),
                                }
                            ).encode()
                        ).hexdigest()
                registered = metadata.tables.get(name)
                if not command.preserve_ids and registered is not None:
                    primary = tuple(registered.primary_key.columns)
                    if (
                        len(primary) == 1
                        and primary[0].name == "id"
                        and isinstance(primary[0].type, sa.Integer)
                    ):
                        values.pop("id", None)
                names = tuple(values)
                await tx.connection.exec_driver_sql(
                    f"INSERT INTO {quoted(name)} ({','.join(quoted(field) for field in names)}) VALUES ({','.join('?' for _ in names)})",
                    tuple(values[field] for field in names),
                )
        execution_data = remap(state.execution.model_dump(mode='json'))
        if not command.preserve_ids:
            for job in execution_data['jobs']:
                job['caller_id'] = ctx.caller_id
        paused = list(await execution.restore_namespace_in(tx, ctx, ExecutionSnapshot.model_validate(execution_data)))
        for raw in original.get("nous_content_references", ()):
            await content.attach_in(
                tx, leases[str(raw["content_hash"])], token(str(raw["owner_ref"]))
            )
        await tx.connection.execute(
            clock.update().where(clock.c.singleton == 1).values(commit_seq=final_seq)
        )
        return NamespaceBackupRestored(
            namespace=ns,
            mapping=mapping,
            resumed_jobs=tuple(paused),
            authority_commit_seq=final_seq,
        )
