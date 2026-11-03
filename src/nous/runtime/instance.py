"""Instance selection and native exclusive ownership; no implicit initialization."""

import os
import tomllib
from collections.abc import Mapping
from dataclasses import asdict, dataclass
from pathlib import Path
from types import TracebackType
from typing import Literal, Self

from filelock import FileLock, Timeout
from platformdirs import PlatformDirs

from nous.common.errors import NousError
from nous.configuration import Configuration


@dataclass(frozen=True)
class InstanceLocator:
    directory: Path

    def __post_init__(self) -> None:
        object.__setattr__(self, "directory", self.directory.absolute().resolve())

    @property
    def config_file(self) -> Path:
        return self.directory / "nous.toml"

    @classmethod
    def resolve(
        cls,
        path: Self | str | Path | None = None,
        environment: Mapping[str, str] | None = None,
    ) -> Self:
        if isinstance(path, InstanceLocator):
            return cls(path.directory)
        environment = os.environ if environment is None else environment
        selected = path or environment.get("NOUS_INSTANCE")
        return cls(
            Path(selected)
            if selected
            else PlatformDirs("nous", appauthor=False).user_config_path / "instances" / "default"
        )


@dataclass(frozen=True)
class InstanceDescriptor:
    instance_id: str
    display_name: str
    layout: Literal["portable", "platform"]


class InstanceLock:
    def __init__(self, instance_directory: Path, *, data: bool = False) -> None:
        self.directory = instance_directory.resolve()
        self._lock = FileLock(
            self.directory / (".nous-data.lock" if data else "instance.lock"), thread_local=False
        )

    def __enter__(self) -> Self:
        if not self.directory.is_dir():
            raise NousError(
                "instance.resource_missing",
                "The instance resource directory does not exist.",
                details={"path": str(self.directory)},
            )
        try:
            self._lock.acquire(timeout=0)
        except Timeout as error:
            raise NousError(
                "instance.in_use",
                "The instance is already in use.",
                details={"path": str(self.directory)},
                cause=error,
            ) from error
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self._lock.release()


def inspect_instance(
    locator: InstanceLocator | Path | str | None = None,
    *,
    environment: Mapping[str, str] | None = None,
) -> dict[str, object]:
    """Inspect files even if SQLite cannot be opened; never repair or create files."""
    from .bindings import InstanceBinding
    from .paths import ResolvedPaths

    selected = InstanceLocator.resolve(locator, environment)
    data_lock: Path | None = None
    result: dict[str, object] = {"config_file": str(selected.config_file), "initialized": False}
    if not selected.config_file.is_file():
        result["diagnostic"] = "Instance configuration does not exist."
        return result
    try:
        with selected.config_file.open("rb") as stream:
            raw = tomllib.load(stream)
        instance = raw.get("instance", {})
        result["instance"] = {
            "instance_id": instance.get("id"),
            "display_name": instance.get("name"),
            "layout": instance.get("layout"),
        }
        candidate = Configuration(
            selected.config_file, environment=environment
        ).snapshot.typed_values()
        descriptor = InstanceDescriptor(
            candidate.instance.id, candidate.instance.name, candidate.instance.layout
        )
        paths = ResolvedPaths.resolve(selected, descriptor, candidate.paths)
        result.update(instance=asdict(descriptor), paths=paths.to_dict())
        binding = InstanceBinding.read(selected)
        binding.check_locations(selected, descriptor, paths, candidate.database.filename)
        data_lock = paths.data / ".nous-data.lock"
        binding.check_resources(paths)
        result.update(initialized=True, database_id=str(binding.database_id))
    except NousError as error:
        result["diagnostic"] = error.message
        result["error"] = error.to_dict()
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        result["diagnostic"] = str(error)
    # Lock ownership can be inspected even when configuration validation failed.
    lock_files = [selected.directory / "instance.lock"]
    if data_lock is not None:
        lock_files.append(data_lock)
    result["in_use"] = False
    try:
        for lock_file in lock_files:
            if lock_file.is_file():
                probe = FileLock(lock_file, thread_local=False)
                try:
                    probe.acquire(timeout=0)
                except Timeout:
                    result["in_use"] = True
                else:
                    probe.release()
    except OSError as error:
        result["lock_diagnostic"] = str(error)
    return result
