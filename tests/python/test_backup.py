"""An installation archive restores the same database/content snapshot and work."""

import asyncio
import zipfile

import pytest
from nous_wave.contracts import CallContext

from nous.platform.content import ContentStore
from nous.platform.execution import DurableExecution, JobSpec
from nous.platform.persistence import Authority, BackupService


async def _backup_job(reference):
    return {"reference": reference}


def test_installation_archive_restores_content_receipts_and_dbos_work(tmp_path):
    async def scenario():
        ctx = CallContext(caller_id="owner", namespace_id="namespace")
        authority = Authority(tmp_path / "original" / "data" / "authority.sqlite")
        content = ContentStore(authority, tmp_path / "original" / "content")
        await authority.initialize()
        execution = DurableExecution(authority)
        execution.register("backup-job", _backup_job)
        await execution.start()
        original = b"immutable original bytes, retained by the exact SQLite snapshot"
        lease = await content.publish_bytes(original, "application/octet-stream")
        async with authority.write(ctx, "adopt", operation_kind="adopt", payload={}) as tx:
            await content.attach_in(tx, lease, "material:original")
            job = await tx.enqueue(
                JobSpec("backup-job", "default", args=("persisted-request",), delay_seconds=60)
            )
            tx.result({"job": job.workflow_id, "material": lease.content_hash})
        await content.release(lease)
        archive = await BackupService(authority, content).create(tmp_path / "installation.zip")
        assert archive.object_count == 1 and archive.commit_seq == 1
        async with content.preserve_all():
            async with authority.write(ctx, "delete", operation_kind="detach", payload={}) as tx:
                await content.detach_in(tx, "material:original")
                tx.result(None)
            assert await content.purge(ctx, (lease.content_hash,)) == ()
        assert await content.purge(ctx, (lease.content_hash,)) == (lease.content_hash,)
        await execution.close()
        await authority.close()

        restored = await BackupService.restore(archive.path, tmp_path / "restored")
        assert restored.commit_seq == 1
        authority = Authority(restored.path / "data" / "authority.sqlite")
        content = ContentStore(authority, restored.path / "content")
        await authority.initialize()
        await content.initialize()
        objects = await content.referenced_objects()
        assert len(objects) == 1
        assert b"".join([chunk async for chunk in content.iter_bytes(objects[0])]) == original
        receipt = await authority.receipt(ctx, "adopt", operation_kind="adopt", payload={})
        assert receipt is not None and receipt.result == {
            "job": job.workflow_id,
            "material": lease.content_hash,
        }
        execution = DurableExecution(authority)
        execution.register("backup-job", _backup_job)
        await execution.start()
        try:
            await execution.resume(ctx, job.workflow_id)
            async with asyncio.timeout(10):
                assert await execution.wait(ctx, job.workflow_id) == {
                    "reference": "persisted-request"
                }
        finally:
            await execution.close()
            await authority.close()
        with pytest.raises(FileExistsError):
            await BackupService.restore(archive.path, restored.path)

        corrupt = tmp_path / "corrupt.zip"
        with zipfile.ZipFile(archive.path) as source, zipfile.ZipFile(corrupt, "x") as destination:
            for entry in source.infolist():
                payload = source.read(entry)
                if entry.filename.startswith("objects/"):
                    payload = b"X" + payload[1:]
                destination.writestr(entry, payload)
        with pytest.raises(ValueError, match="SHA-256"):
            await BackupService.restore(corrupt, tmp_path / "rejected")
        assert not (tmp_path / "rejected").exists()

    asyncio.run(scenario())
