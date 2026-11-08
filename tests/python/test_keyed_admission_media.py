"""Real PDF conversion must not invert its outer lock and admission."""

import asyncio
import io
from pathlib import Path

import pypdfium2 as pdfium

from nous.content.contracts import SourceSelection
from nous.media.contracts import PreparedBatch
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_direct_and_nested_pdf_materialization_share_one_slot_without_deadlock(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        output = io.BytesIO()
        document = pdfium.PdfDocument.new()
        try:
            page = document.new_page(width=90, height=90)
            page.close()
            document.save(output)
        finally:
            document.close()
        async with open_instance(
            instance,
            environment={},
            overrides={"execution.max_in_flight": 1, "execution.default_timeout_seconds": None},
        ) as runtime:
            original = await runtime.artifacts.ingest(
                output.getvalue(), media_type="application/pdf", display_name="sample.pdf"
            )
            settings = runtime.configuration.snapshot.values.media
            plan = await runtime.media.preparer.prepare(
                original.ref,
                SourceSelection(pages=(1,)),
                settings=settings,
                document_mode="visual",
            )
            admitted, release = asyncio.Event(), asyncio.Event()

            async def nested() -> PreparedBatch:
                async with runtime.execution.operation("composed.pdf"):
                    admitted.set()
                    await release.wait()
                    return await runtime.media.preparer.materialize(plan.batches[0])

            parent = asyncio.create_task(nested())
            direct = None
            try:
                await asyncio.wait_for(admitted.wait(), 5)
                direct = asyncio.create_task(runtime.media.preparer.materialize(plan.batches[0]))
                for _ in range(400):
                    if runtime.execution.status()["queued"] == 1:
                        break
                    await asyncio.sleep(0.005)
                assert runtime.execution.status()["queued"] == 1
                release.set()
                first, second = await asyncio.wait_for(asyncio.gather(parent, direct), 5)
                assert first is not None and second is not None
            finally:
                release.set()
                for task in (parent, direct):
                    if task is not None and not task.done():
                        task.cancel()
                await asyncio.gather(
                    *(task for task in (parent, direct) if task is not None),
                    return_exceptions=True,
                )

    asyncio.run(scenario())
