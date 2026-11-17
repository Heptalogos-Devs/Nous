"""Protect the local control capability with Windows per-user DPAPI."""

from __future__ import annotations

import base64
import ctypes
from collections.abc import Callable
from typing import cast

from nous.common.errors import NousError


class _Blob(ctypes.Structure):
    _fields_ = [("size", ctypes.c_uint32), ("data", ctypes.c_void_p)]
    size: int
    data: int | None


def _crypt(data: bytes, *, encrypt: bool) -> bytes:
    """Use user-scoped DPAPI, forbid UI, and release the OS-owned result buffer."""
    try:
        load = cast(Callable[..., ctypes.CDLL], ctypes.WinDLL)
        crypt32 = load("crypt32", use_last_error=True)
        kernel32 = load("kernel32", use_last_error=True)
        operation = crypt32.CryptProtectData if encrypt else crypt32.CryptUnprotectData
        operation.argtypes = [
            ctypes.POINTER(_Blob),
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.POINTER(_Blob),
        ]
        operation.restype = ctypes.c_int
        local_free = kernel32.LocalFree
        local_free.argtypes = [ctypes.c_void_p]
        local_free.restype = ctypes.c_void_p
        buffer = ctypes.create_string_buffer(data)
        source = _Blob(len(data), ctypes.cast(buffer, ctypes.c_void_p))
        result = _Blob()
        if not operation(ctypes.byref(source), None, None, None, None, 1, ctypes.byref(result)):
            raise OSError(ctypes.get_last_error(), "Windows user data protection failed")
        pointer = result.data
        if pointer is None:
            raise OSError("Windows user data protection returned no data")
        try:
            return ctypes.string_at(pointer, result.size)
        finally:
            ctypes.memset(pointer, 0, result.size)
            local_free(pointer)
    except (OSError, ValueError, AttributeError) as error:
        raise NousError(
            "instance.control_unavailable",
            "Windows user data protection is unavailable",
            cause=error,
        ) from error


def protect(secret: str) -> str:
    return base64.b64encode(_crypt(bytes.fromhex(secret), encrypt=True)).decode("ascii")


def unprotect(value: str) -> str:
    try:
        encrypted = base64.b64decode(value, validate=True)
    except ValueError as error:
        raise NousError(
            "instance.control_insecure", "Invalid protected local control capability", cause=error
        ) from error
    secret = _crypt(encrypted, encrypt=False)
    if len(secret) != 32:
        raise NousError("instance.control_insecure", "Invalid protected local control capability")
    return secret.hex()
