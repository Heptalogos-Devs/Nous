"""Isolate local CLI streams and paths for concurrently served invocations."""

import io
import os
import sys
from collections.abc import Callable, Generator, Iterable, Mapping
from contextlib import contextmanager
from pathlib import Path
from typing import TYPE_CHECKING, NoReturn, cast

import click
from typer.models import TyperPath

from nous.runtime.local_context import current_call

if TYPE_CHECKING:
    from typing import TextIO


class ClientInput(io.StringIO):
    def __init__(self, text: str | None, is_tty: bool, read_input: Callable[[], str]) -> None:
        super().__init__(text or "")
        self._is_tty, self._read_input = is_tty, read_input
        self._loaded = text is not None or is_tty

    def _load(self) -> None:
        if not self._loaded:
            self._loaded = True
            self.write(self._read_input())
            self.seek(0)

    def read(self, size: int | None = -1) -> str:
        self._load()
        return super().read(-1 if size is None else size)

    def readline(self, size: int = -1) -> str:
        self._load()
        return super().readline(size)

    def isatty(self) -> bool:
        return self._is_tty


class ForwardOutput(io.TextIOBase):
    """Forward output while the command is running, preserving its destination."""

    def __init__(self, write_output: Callable[[str], None]) -> None:
        self._write_output = write_output
        self._total = 0

    def write(self, value: str) -> int:
        self._total += len(value.encode("utf-8"))
        if self._total > 64 * 1024 * 1024:
            raise ValueError("CLI output exceeds the local control limit")
        if value:
            self._write_output(value)
        return len(value)

    def flush(self) -> None:
        pass


class StreamProxy:
    def __init__(self, original: TextIO, name: str) -> None:
        self.original, self.name = original, name

    def _selected(self) -> TextIO:
        call = current_call.get()
        return getattr(call, self.name) if call is not None and not call.closed else self.original

    def write(self, value: str) -> int:
        return self._selected().write(value)

    def read(self, size: int = -1) -> str:
        return self._selected().read(size)

    def readline(self, size: int = -1) -> str:
        return self._selected().readline(size)

    def flush(self) -> None:
        self._selected().flush()

    def isatty(self) -> bool:
        return self._selected().isatty()

    @property
    def encoding(self) -> str:
        return self.original.encoding or "utf-8"

    def __getattr__(self, name: str) -> object:
        return cast(object, getattr(self._selected(), name))


@contextmanager
def isolated_cli_streams() -> Generator[None]:
    """Route only the request's context; unrelated host work keeps its streams."""
    stdin, stdout, stderr = sys.stdin, sys.stdout, sys.stderr
    convert, typer_convert, edit = click.Path.convert, TyperPath.convert, click.edit
    typer_convert_typed = cast(
        Callable[
            [TyperPath, str | os.PathLike[str], object, object], str | bytes | os.PathLike[str]
        ],
        typer_convert,
    )

    def client_path(value: str | os.PathLike[str]) -> str | os.PathLike[str]:
        call = current_call.get()
        if call is not None and str(value) != "-":
            path = Path(value)
            if not path.is_absolute():
                return str(call.cwd / path)
        return value

    def validate_path(
        self: click.Path | TyperPath,
        converted: str | os.PathLike[str],
        param: object,
        ctx: object,
    ) -> str | bytes | os.PathLike[str]:
        call = current_call.get()
        assert call is not None
        info = call.path_info_sync(Path(converted), resolve=self.resolve_path)
        inspected = info.get("path")
        if not isinstance(inspected, str) or not isinstance(info.get("exists"), bool):
            raise ValueError("Invalid client path information")
        fail = cast(Callable[[str, object, object], NoReturn], self.fail)
        label = self.name.title()
        location = repr(click.format_filename(inspected))
        if not info["exists"]:
            if self.exists:
                fail(f"{label} {location} does not exist.", param, ctx)
            return self.coerce_path_result(inspected)
        if not self.file_okay and info.get("regular") is True:
            fail(f"{label} {location} is a file.", param, ctx)
        if not self.dir_okay and info.get("directory") is True:
            fail(f"{label} {location} is a directory.", param, ctx)
        if self.readable and info.get("readable") is not True:
            fail(f"{label} {location} is not readable.", param, ctx)
        if self.writable and info.get("writable") is not True:
            fail(f"{label} {location} is not writable.", param, ctx)
        if getattr(self, "executable", False) and info.get("executable") is not True:
            fail(f"{label} {location} is not executable.", param, ctx)
        return self.coerce_path_result(inspected)

    def convert_path(
        self: click.Path,
        value: str | os.PathLike[str],
        param: click.Parameter | None,
        ctx: click.Context | None,
    ) -> str | bytes | os.PathLike[str]:
        converted = client_path(value)
        if (
            current_call.get() is None
            or getattr(param, "name", None) == "instance"
            or str(converted) == "-"
        ):
            return convert(self, converted, param, ctx)
        return validate_path(self, converted, param, ctx)

    def convert_typer_path(
        self: TyperPath, value: str | os.PathLike[str], param: object, ctx: object
    ) -> str | bytes | os.PathLike[str]:
        converted = client_path(value)
        if (
            current_call.get() is None
            or getattr(param, "name", None) == "instance"
            or str(converted) == "-"
        ):
            return typer_convert_typed(self, converted, param, ctx)
        return validate_path(self, converted, param, ctx)

    def client_editor(
        text: str | bytes | bytearray | None = None,
        editor: str | None = None,
        env: Mapping[str, str] | None = None,
        require_save: bool = True,
        extension: str = ".txt",
        filename: str | os.PathLike[str] | Iterable[str | os.PathLike[str]] | None = None,
    ) -> str | bytes | bytearray | None:
        call = current_call.get()
        if call is None:
            return edit(text, editor, env, require_save, extension, filename)  # pyright: ignore[reportArgumentType]
        if (
            text is not None
            or editor is not None
            or env is not None
            or not isinstance(filename, (str, os.PathLike))
        ):
            raise ValueError(
                "Local CLI editor requires a single file and the client editor configuration"
            )
        call.edit(os.fspath(filename))
        return None

    sys.stdin = StreamProxy(stdin, "stdin")  # type: ignore[assignment]
    sys.stdout = StreamProxy(stdout, "stdout")  # type: ignore[assignment]
    sys.stderr = StreamProxy(stderr, "stderr")  # type: ignore[assignment]
    click.Path.convert = convert_path  # type: ignore[method-assign]
    TyperPath.convert = convert_typer_path  # type: ignore[method-assign]
    click.edit = client_editor
    try:
        yield
    finally:
        click.Path.convert, TyperPath.convert, click.edit = convert, typer_convert, edit  # type: ignore[method-assign]
        sys.stdin, sys.stdout, sys.stderr = stdin, stdout, stderr
