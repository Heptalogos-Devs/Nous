"""Portable semantic archives of one Namespace."""
from typing import Literal
from pydantic import JsonValue
from .common import Contract, Operation
from .identity import Namespace, SubjectBinding
from .management import NamespaceBundle

class NamespaceArchive(Contract):
    format: Literal['nous.namespace'] = 'nous.namespace'
    version: Literal[3] = 3
    namespace: Namespace
    authority_commit_seq: int
    bundle: NamespaceBundle
    bindings: tuple[SubjectBinding, ...] = ()
    configuration_toml: str

class RestoreNamespace(Operation):
    archive: NamespaceArchive
    slug: str | None = None
    display_name: str | None = None

class NamespaceRestoreResult(Contract):
    namespace: Namespace
    installation: dict[str, JsonValue]
