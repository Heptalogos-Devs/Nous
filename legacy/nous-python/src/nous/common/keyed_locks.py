"""Bound keyed locks to active or waiting work in the owning event loop."""

import asyncio
from collections.abc import AsyncGenerator, Hashable
from contextlib import asynccontextmanager
from dataclasses import dataclass, field


@dataclass
class _Entry:
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    users: int = 0


class KeyedLocks[K: Hashable]:
    def __init__(self) -> None:
        self._entries: dict[K, _Entry] = {}

    def active_keys(self) -> set[K]:
        return set(self._entries)

    @asynccontextmanager
    async def hold(self, key: K) -> AsyncGenerator[None]:
        entry = self._entries.setdefault(key, _Entry())
        entry.users += 1
        try:
            async with entry.lock:
                yield
        finally:
            entry.users -= 1
            if entry.users == 0:
                del self._entries[key]
