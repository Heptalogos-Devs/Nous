"""Instance selection and native exclusive ownership; no implicit initialization."""

import os
import tomllib
from collections.abc import Mapping
from dataclasses import asdict, dataclass
from pathlib import Path
from types import TracebackType
from typing import Literal, Self

import tomlkit
from filelock import FileLock, Timeout
from platformdirs import PlatformDirs
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from nous.common.errors import NousError
from nous.common.files import write_text
from nous.configuration import Configuration


class _ProfileValues(BaseModel):
    model_config = ConfigDict(extra="forbid")
    current: str | None = None
    profiles: dict[str, str] = Field(default_factory=dict)


class InstanceProfiles:
    """Own the client-side names and selection; registered instances keep their own state."""

    def __init__(self, file: Path | None = None) -> None:
        self.file = file or PlatformDirs("nous", appauthor=False).user_config_path / "client.toml"

    def _read(self) -> _ProfileValues:
        if not self.file.exists():
            return _ProfileValues()
        try:
            document = tomllib.loads(self.file.read_text(encoding="utf-8"))
            return _ProfileValues.model_validate(document.get("instances", {}))
        except (OSError, ValueError, ValidationError) as error:
            raise NousError(
                "instance.profiles_invalid",
                f"Cannot read instance profiles: {error}",
                details={"file": str(self.file)},
            ) from error

    def _save(self, values: _ProfileValues) -> None:
        try:
            if self.file.exists():
                with self.file.open("r", encoding="utf-8", newline="") as stream:
                    document = tomlkit.parse(stream.read())
            else:
                document = tomlkit.document()
        except (OSError, ValueError) as error:
            raise NousError(
                "instance.profiles_invalid",
                f"Cannot read instance profiles: {error}",
                details={"file": str(self.file)},
            ) from error
        document["instances"] = values.model_dump(exclude_none=True)
        write_text(self.file, tomlkit.dumps(document))

    def resolve(self, name: str) -> Path:
        values = self._read()
        if name not in values.profiles:
            raise NousError("instance.profile_missing", f"Unknown instance profile: {name}")
        directory = Path(values.profiles[name]).expanduser().resolve()
        if not (directory / "nous.toml").is_file():
            raise NousError(
                "instance.profile_path_invalid",
                f"Instance profile {name} has no configuration",
                details={"directory": str(directory), "profile": name},
            )
        return directory

    def register(self, name: str, directory: Path) -> dict[str, object]:
        if not name or name.strip() != name or any(char in name for char in "/\\"):
            raise NousError(
                "instance.profile_name_invalid",
                "Profile names must be nonempty names without path separators",
            )
        locator = InstanceLocator(directory)
        state = inspect_instance(locator)
        if not state["initialized"]:
            raise NousError(
                "instance.profile_path_invalid",
                "Register an initialized instance configuration directory",
                details=state,
            )
        values = self._read()
        values.profiles[name] = str(locator.directory)
        self._save(values)
        return {
            "profile": name,
            "directory": str(locator.directory),
            "current": values.current == name,
        }

    def use(self, name: str) -> dict[str, object]:
        directory = self.resolve(name)
        values = self._read()
        values.current = name
        self._save(values)
        return {"profile": name, "directory": str(directory), "current": True}

    def current(self) -> str | None:
        return self._read().current


@dataclass(frozen=True)
class InstanceLocator:
    directory: Path
    profile: str | None = None

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
        *,
        profile: str | None = None,
    ) -> Self:
        if path is not None and profile is not None:
            raise NousError("instance.selection_conflict", "Choose --instance or --profile")
        if isinstance(path, InstanceLocator):
            return cls(path.directory, path.profile)
        environment = os.environ if environment is None else environment
        profiles = InstanceProfiles()
        if profile is not None:
            return cls(profiles.resolve(profile), profile)
        selected = path or environment.get("NOUS_INSTANCE")
        if selected:
            return cls(Path(selected))
        current = profiles.current()
        if current is not None:
            return cls(profiles.resolve(current), current)
        return cls(PlatformDirs("nous", appauthor=False).user_config_path / "instances" / "default")


@dataclass(frozen=True)
class InstanceDescriptor:
    instance_id: str
    display_name: str
    layout: Literal["portable", "platform"]


class InstanceLock:
    """Hold the instance or data-directory lock for the context lifetime.

    Acquisition is nonblocking and requires the directory to exist. A held lock
    raises ``instance.in_use``; context exit releases an acquired lock.
    """

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
        candidate = Configuration(selected.config_file, environment=environment).snapshot.values
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
