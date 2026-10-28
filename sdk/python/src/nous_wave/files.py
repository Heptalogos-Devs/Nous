import json
from pathlib import Path, PureWindowsPath
from uuid import uuid4

import anyio

from .types import ResolvedPath


def file_name(value: object) -> str:
    if (
        not isinstance(value, str)
        or not value
        or value in {".", ".."}
        or any(char in value for char in '/\\:\x00<>"|?*')
        or Path(value).name != value
        or PureWindowsPath(value).is_reserved()  # pyright: ignore[reportDeprecated]
        or value.endswith((".", " "))
    ):
        raise ValueError("file_name must be a plain file name")
    return value


class ManagedFiles:
    def __init__(self, root: str | Path) -> None:
        path = Path(root)
        if not path.is_absolute():
            raise ValueError("transfer_root must be an absolute caller-owned directory")
        self.root = path.resolve()

    def contained(self, path: str | Path, *, must_exist: bool = False) -> Path:
        candidate = Path(path)
        candidate = (candidate if candidate.is_absolute() else self.root / candidate).resolve(
            strict=must_exist
        )
        if not candidate.is_relative_to(self.root):
            raise ValueError("File transfer path escapes transfer_root")
        return candidate

    def local_file(self, path: str | Path) -> Path:
        candidate = self.contained(path, must_exist=True)
        if not candidate.is_file():
            raise ValueError("File transfer input must be a regular file")
        return candidate

    def path_aliases(self) -> dict[str, str]:
        return {
            name: str(self.contained(path))
            for name, path in {
                "raw": "source/raw",
                "lpmm": "source/lpmm",
                "maibot": "source/maibot",
                "converted": "converted",
            }.items()
        }

    @staticmethod
    def relative_path(value: str) -> Path:
        path = PureWindowsPath(value)
        if (
            path.drive
            or path.root
            or ".." in path.parts
            or any(":" in part or "\x00" in part for part in path.parts)
        ):
            raise ValueError("Alias paths must be relative and remain within the selected alias")
        return Path(*path.parts)

    def resolve_alias(
        self, alias: str, relative_path: str = "", *, must_exist: bool = True
    ) -> Path:
        aliases = self.path_aliases()
        if alias not in aliases:
            raise ValueError(f"Unknown file transfer alias: {alias}")
        root = Path(aliases[alias])
        candidate = self.contained(root / self.relative_path(relative_path), must_exist=must_exist)
        if not candidate.is_relative_to(root):
            raise ValueError("File transfer path escapes the selected alias")
        return candidate

    def resolve_path(
        self, alias: str, relative_path: str = "", *, must_exist: bool = True
    ) -> ResolvedPath:
        path = self.resolve_alias(alias, relative_path, must_exist=must_exist)
        return ResolvedPath(
            alias=alias,
            relative_path=relative_path,
            resolved_path=path,
            exists=path.exists(),
            is_file=path.is_file(),
            is_dir=path.is_dir(),
        )

    def file_identity(self, alias: str, path: str | Path) -> str:
        """Return an opaque caller identity without exposing an absolute host path."""
        root = self.resolve_alias(alias)
        selected = self.local_file(path)
        if not selected.is_relative_to(root):
            raise ValueError("Import file escapes the selected alias")
        return json.dumps(
            [alias, selected.relative_to(root).as_posix()],
            ensure_ascii=False,
            separators=(",", ":"),
        )

    async def scan(
        self, alias: str, relative_path: str = "", *, pattern: str = "*", recursive: bool = True
    ) -> list[Path]:
        self.relative_path(pattern)

        def collect() -> list[Path]:
            root = self.resolve_alias(alias)
            selected = self.resolve_alias(alias, relative_path)
            if selected.is_file():
                candidates = [selected]
            else:
                candidates = selected.rglob(pattern) if recursive else selected.glob(pattern)
            paths: set[Path] = set()
            for candidate in candidates:
                if not candidate.is_file():
                    continue
                if candidate.suffix.lower() not in {".txt", ".md", ".json", ".jsonl"}:
                    continue
                path = self.local_file(candidate)
                if not path.is_relative_to(root):
                    raise ValueError("Scanned file escapes the selected alias")
                paths.add(path)
            return sorted(paths, key=lambda path: path.as_posix())

        return await anyio.to_thread.run_sync(collect)

    async def download_destination(self, name: str) -> Path:
        name = file_name(name)
        downloads = self.contained("downloads")
        await anyio.Path(downloads).mkdir(parents=True, exist_ok=True)
        directory = self.contained(downloads / uuid4().hex)
        await anyio.Path(directory).mkdir(exist_ok=False)
        return directory / name
