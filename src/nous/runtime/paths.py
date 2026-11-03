"""Resolve physical directories once for each selected instance."""

from collections.abc import Mapping
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Literal

from platformdirs import PlatformDirs

from nous.configuration.definitions import PathsConfig

from .errors import NousError
from .instance import InstanceDescriptor, InstanceLocator

PathKind = Literal[
    "config", "data", "content", "index", "cache", "logs", "temp", "runtime", "exports"
]


@dataclass(frozen=True)
class ResolvedPaths:
    config: Path
    data: Path
    content: Path
    index: Path
    cache: Path
    logs: Path
    temp: Path
    runtime: Path
    exports: Path

    @classmethod
    def resolve(
        cls,
        locator: InstanceLocator,
        descriptor: InstanceDescriptor,
        overrides: Mapping[str, object] | PathsConfig,
    ) -> ResolvedPaths:
        root = locator.directory
        if descriptor.layout == "portable":
            defaults = {
                kind: root / kind
                for kind in (
                    "data",
                    "content",
                    "index",
                    "cache",
                    "logs",
                    "temp",
                    "runtime",
                    "exports",
                )
            }
        else:
            platform = PlatformDirs("nous", appauthor=False)
            suffix = Path("instances") / descriptor.instance_id
            data = platform.user_data_path / suffix
            defaults = {
                "data": data,
                "content": data / "content",
                "index": data / "index",
                "exports": data / "exports",
                "cache": platform.user_cache_path / suffix,
                "logs": platform.user_log_path / suffix,
                "runtime": platform.user_state_path / suffix / "runtime",
                "temp": platform.user_cache_path / suffix / "temp",
            }
        resolved: dict[str, Path] = {"config": root}
        for kind, default in defaults.items():
            key = kind + "_dir"
            value: object = (
                overrides.get(key)
                if isinstance(overrides, Mapping)
                else getattr(overrides, key, None)
            )
            path = Path(str(value)) if value is not None else default
            resolved[kind] = (root / path if not path.is_absolute() else path).resolve()
        return cls(**resolved)

    def for_kind(self, kind: PathKind) -> Path:
        return getattr(self, kind)

    def resolve_owned(self, kind: PathKind, relative: str | Path) -> Path:
        item = Path(relative)
        root = self.for_kind(kind)
        candidate = (root / item).resolve()
        if item.is_absolute() or not candidate.is_relative_to(root):
            raise NousError(
                "storage.invalid_path",
                "The relative path escapes its storage directory.",
                details={"kind": kind, "path": str(relative)},
            )
        return candidate

    def create_runtime_directories(self) -> None:
        for kind in ("cache", "logs", "temp", "runtime", "exports"):
            getattr(self, kind).mkdir(parents=True, exist_ok=True)

    def to_dict(self) -> dict[str, str]:
        return {key: str(value) for key, value in asdict(self).items()}
