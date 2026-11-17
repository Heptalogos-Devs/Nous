"""Length-bounded local-only transport and per-user endpoint identity."""

import asyncio
import hashlib
import hmac
import json
import os
import socket
import stat
import struct
import tempfile
from pathlib import Path
from typing import Literal, TypedDict, cast

from platformdirs import PlatformDirs

from nous.common.errors import NousError
from nous.runtime.instance import InstanceLocator

MAX_FRAME = 64 * 1024 * 1024


class Endpoint(TypedDict):
    host: Literal["127.0.0.1"]
    port: int
    secret: str


def endpoint(locator: InstanceLocator, *, create: bool = False) -> Path | None:
    if os.name == "nt":
        directory = PlatformDirs("nous", appauthor=False).user_cache_path / "control"
        if create:
            directory.mkdir(parents=True, exist_ok=True)
        if (
            directory.is_symlink()
            or directory.is_junction()
            or (directory.exists() and not directory.is_dir())
        ):
            raise NousError("instance.control_insecure", "Invalid local control directory")
    else:
        parent = Path("/tmp") if Path("/tmp").is_dir() else Path(tempfile.gettempdir())
        directory = parent / f"nous-control-{os.getuid()}"
        if create:
            try:
                directory.mkdir(mode=0o700)
            except FileExistsError:
                pass
        if directory.exists() or directory.is_symlink():
            info = directory.lstat()
            if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise NousError(
                    "instance.control_insecure", "Local control directory is not private"
                )
    digest = hashlib.sha256(os.fsencode(os.path.normcase(str(locator.directory)))).hexdigest()
    return directory / f"{digest}.json"


def read_endpoint(path: Path) -> Endpoint | None:
    try:
        if os.name == "nt" and (path.is_symlink() or path.is_junction()):
            raise NousError("instance.control_insecure", "Invalid local control endpoint")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags)
    except FileNotFoundError:
        return None
    except OSError as error:
        raise NousError(
            "instance.control_insecure",
            "Cannot safely open the local control endpoint",
            cause=error,
        ) from error
    with os.fdopen(descriptor, "rb") as stream:
        info = os.fstat(stream.fileno())
        if (
            not stat.S_ISREG(info.st_mode)
            or (os.name != "nt" and (info.st_uid != os.getuid() or info.st_mode & 0o077))
            or info.st_size > 4096
        ):
            raise NousError("instance.control_insecure", "Local control endpoint is not private")
        try:
            value = cast(object, json.load(stream))
        except (UnicodeError, ValueError) as error:
            raise NousError(
                "instance.control_insecure", "Invalid local control endpoint", cause=error
            ) from error
    if not isinstance(value, dict):
        raise NousError("instance.control_insecure", "Invalid local control endpoint")
    value = cast(dict[str, object], value)
    port, secret = value.get("port"), value.get("secret")
    if (
        value.get("host") != "127.0.0.1"
        or not isinstance(port, int)
        or not 1 <= port <= 65535
        or not isinstance(secret, str)
    ):
        raise NousError("instance.control_insecure", "Invalid local control endpoint")
    if os.name == "nt":
        from .control_windows import unprotect

        secret = unprotect(secret)
    if len(secret) != 64:
        raise NousError("instance.control_insecure", "Invalid local control capability")
    return Endpoint(host="127.0.0.1", port=port, secret=secret)


def proof(secret: str, role: str, *nonces: str) -> str:
    return hmac.new(
        bytes.fromhex(secret), (role + ":" + ":".join(nonces)).encode("ascii"), hashlib.sha256
    ).hexdigest()


def encode(value: object) -> bytes:
    data = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(data) > MAX_FRAME:
        raise NousError("instance.control_too_large", "Local control message exceeds its limit")
    return struct.pack("!I", len(data)) + data


async def receive(reader: asyncio.StreamReader, *, limit: int = MAX_FRAME) -> dict[str, object]:
    size = struct.unpack("!I", await reader.readexactly(4))[0]
    if size > limit:
        raise NousError("instance.control_too_large", "Local control message exceeds its limit")
    value = cast(object, json.loads(await reader.readexactly(size)))
    if not isinstance(value, dict):
        raise ValueError("Invalid local control message")
    return cast(dict[str, object], value)


async def send(writer: asyncio.StreamWriter, value: object) -> None:
    writer.write(encode(value))
    await writer.drain()


def receive_sync(sock: socket.socket) -> dict[str, object]:
    def exact(size: int) -> bytes:
        parts = bytearray()
        while len(parts) < size:
            part = sock.recv(size - len(parts))
            if not part:
                raise EOFError("Local host connection closed")
            parts.extend(part)
        return bytes(parts)

    size = struct.unpack("!I", exact(4))[0]
    if size > MAX_FRAME:
        raise NousError("instance.control_too_large", "Local control message exceeds its limit")
    value = cast(object, json.loads(exact(size)))
    if not isinstance(value, dict):
        raise ValueError("Invalid local control message")
    return cast(dict[str, object], value)
