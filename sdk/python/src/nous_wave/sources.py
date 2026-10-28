"""Named source management on the native source owner."""

from collections.abc import Mapping, Sequence
from typing import Any
from uuid import uuid4


class SourceManagement:
    async def sources(self, request: Mapping[str, Any]) -> dict:
        """Manage named source groups through the native list/trash/restore contract."""
        return await self.request("POST", "/api/sources", body=dict(request))

    async def preview_sources(self, namespace_id: str, subject_id: str, sources: Sequence[str], *,
                              force: bool = False, reason: str = "") -> dict:
        """Return the current uncommitted source trash plan."""
        return await self.sources({"action": "preview", "request": {
            "namespace_id": namespace_id, "subject_id": subject_id,
            "sources": list(sources), "force": force, "reason": reason}})

    async def source_operation(self, namespace_id: str, subject_id: str, commit_seq: int) -> dict:
        return await self.sources({"action": "get", "request": {
            "namespace_id": namespace_id, "subject_id": subject_id, "commit_seq": commit_seq}})

    async def source_operations(self, namespace_id: str, subject_id: str, *,
                                limit: int = 50, before_commit_seq: int | None = None) -> dict:
        return await self.sources({"action": "list_operations", "request": {
            "namespace_id": namespace_id, "subject_id": subject_id,
            "limit": limit, "before_commit_seq": before_commit_seq}})

    async def source_directory(self, namespace_id: str, subject_id: str) -> dict:
        return await self.sources({"action": "list", "request": {
            "namespace_id": namespace_id, "subject_id": subject_id}})

    async def trash_sources(self, namespace_id: str, subject_id: str, sources: Sequence[str], *,
                            force: bool = False, reason: str = "", operation_id: str | None = None) -> dict:
        return await self.sources({"action": "trash", "operation_id": operation_id or str(uuid4()),
            "request": {"namespace_id": namespace_id, "subject_id": subject_id,
                        "sources": list(sources), "force": force, "reason": reason}})

    async def restore_sources(self, namespace_id: str, subject_id: str, trash_commit_seq: int, *,
                              reason: str = "", operation_id: str | None = None) -> dict:
        return await self.sources({"action": "restore", "operation_id": operation_id or str(uuid4()),
            "request": {"namespace_id": namespace_id, "subject_id": subject_id,
                        "trash_commit_seq": trash_commit_seq, "reason": reason}})

