"""Durable instance-to-resource identity, written only by explicit initialization."""

from pathlib import Path
from typing import Literal
from uuid import UUID, uuid4

from pydantic import BaseModel, ConfigDict, ValidationError

from nous.common.errors import NousError

from .instance import InstanceDescriptor, InstanceLocator
from .paths import ResolvedPaths

ResourceKind = Literal["data", "content", "index"]
RESOURCE_KINDS: tuple[ResourceKind, ...] = ("data", "content", "index")
INSTANCE_BINDING_FILE = ".nous-instance.json"


class _BindingValue(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class ResourceBinding(_BindingValue):
    resource_id: UUID
    location: str


class ResourceMarker(_BindingValue):
    instance_id: UUID
    resource_id: UUID
    kind: ResourceKind


def _location(root: Path, path: Path) -> str:
    if path.is_relative_to(root):
        return "relative:" + path.relative_to(root).as_posix()
    return "absolute:" + path.as_posix()


def _write_exclusive(path: Path, value: BaseModel) -> None:
    try:
        with path.open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(value.model_dump_json(indent=2) + "\n")
    except FileExistsError as error:
        raise NousError(
            "instance.binding_exists",
            "Instance resource metadata already exists.",
            details={"path": str(path)},
        ) from error


class InstanceBinding(_BindingValue):
    instance_id: UUID
    database_id: UUID
    database_filename: str
    layout: Literal["portable", "platform"]
    data: ResourceBinding
    content: ResourceBinding
    index: ResourceBinding

    @classmethod
    def create(
        cls,
        locator: InstanceLocator,
        descriptor: InstanceDescriptor,
        paths: ResolvedPaths,
        filename: str,
    ) -> InstanceBinding:
        return cls(
            instance_id=UUID(descriptor.instance_id),
            database_id=uuid4(),
            database_filename=filename,
            layout=descriptor.layout,
            **{
                kind: ResourceBinding(
                    resource_id=uuid4(), location=_location(locator.directory, paths.for_kind(kind))
                )
                for kind in RESOURCE_KINDS
            },
        )

    @classmethod
    def read(cls, locator: InstanceLocator) -> InstanceBinding:
        path = locator.directory / INSTANCE_BINDING_FILE
        try:
            return cls.model_validate_json(path.read_bytes())
        except FileNotFoundError as error:
            raise NousError(
                "instance.incomplete",
                "Instance resource metadata is missing; initialization is incomplete.",
                details={"path": str(path)},
            ) from error
        except (ValidationError, ValueError) as error:
            raise NousError(
                "instance.binding_invalid",
                "Instance resource metadata is invalid.",
                details={"path": str(path)},
            ) from error

    def check_locations(
        self,
        locator: InstanceLocator,
        descriptor: InstanceDescriptor,
        paths: ResolvedPaths,
        filename: str,
    ) -> None:
        if self.instance_id != UUID(descriptor.instance_id) or self.layout != descriptor.layout:
            raise NousError(
                "instance.binding_conflict",
                "Instance identity conflicts with its initialized resources.",
            )
        changed = [
            kind
            for kind in RESOURCE_KINDS
            if getattr(self, kind).location != _location(locator.directory, paths.for_kind(kind))
        ]
        if filename != self.database_filename:
            changed.append("database.filename")
        if changed:
            raise NousError(
                "configuration.relocation_required",
                "Existing instance paths require explicit relocation.",
                details={"paths": changed},
            )

    def check_resources(self, paths: ResolvedPaths) -> None:
        for kind in RESOURCE_KINDS:
            path = paths.for_kind(kind) / f".nous-{kind}.json"
            try:
                marker = ResourceMarker.model_validate_json(path.read_bytes())
            except FileNotFoundError as error:
                raise NousError(
                    "instance.resource_missing",
                    "An initialized resource directory or identity marker is missing.",
                    details={"kind": kind, "path": str(path)},
                ) from error
            except (ValidationError, ValueError) as error:
                raise NousError(
                    "instance.binding_invalid",
                    "Resource identity metadata is invalid.",
                    details={"kind": kind, "path": str(path)},
                ) from error
            expected: ResourceBinding = getattr(self, kind)
            if (
                marker.instance_id != self.instance_id
                or marker.resource_id != expected.resource_id
                or marker.kind != kind
            ):
                raise NousError(
                    "instance.binding_conflict",
                    "A resource directory does not belong to this instance.",
                    details={"kind": kind, "path": str(path)},
                )

    def write(self, locator: InstanceLocator, paths: ResolvedPaths) -> None:
        for kind in RESOURCE_KINDS:
            binding: ResourceBinding = getattr(self, kind)
            _write_exclusive(
                paths.for_kind(kind) / f".nous-{kind}.json",
                ResourceMarker(
                    instance_id=self.instance_id, resource_id=binding.resource_id, kind=kind
                ),
            )
        _write_exclusive(locator.directory / INSTANCE_BINDING_FILE, self)


def prepare_resource_directories(paths: ResolvedPaths) -> None:
    """Initialize only unclaimed roots, allowing configured roots nested in each other."""
    roots = tuple(paths.for_kind(kind) for kind in RESOURCE_KINDS)
    for root in roots:
        if root.exists():
            for child in root.iterdir():
                if not child.is_dir() or not any(
                    other != root and other.is_relative_to(child) for other in roots
                ):
                    raise NousError(
                        "instance.resource_not_empty",
                        "A resource directory already contains files.",
                        details={"path": str(root)},
                    )
    for root in roots:
        root.mkdir(parents=True, exist_ok=True)
