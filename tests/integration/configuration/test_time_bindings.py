"""The local runtime's user-visible path and its persistent contracts."""

import asyncio
import hashlib
import io
from fractions import Fraction
from pathlib import Path

import numpy as np
import pytest

from nous.common.errors import NousError
from nous.content.contracts import (
    ContentSelection,
    InputGroup,
    InputPart,
    OutputContent,
    ProgramProducer,
    ResultFacts,
    SelectedInputs,
    SelectedSource,
)
from nous.models.embeddings import resolved_space
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.temporal.domains import TimeDomainDefinition, register_domain


def test_runtime_snapshots_transactions_and_time_domain_bindings(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance)
        async with open_instance(instance) as runtime:
            old = runtime.configuration.snapshot
            result = await runtime.configuration.update({"execution.default_timeout_seconds": 2})
            assert result["state"] == "applied"
            assert old.values.execution.default_timeout_seconds is None
            assert runtime.configuration.snapshot.values.execution.default_timeout_seconds == 2

            mars = {"kind": "linear", "name": "Mars", "tick_seconds": "1/1000", "epoch": "sol 0"}
            assert (await runtime.configuration.update({"time.domains.mars": mars}))[
                "state"
            ] == "applied"
            assert (await runtime.time.domains())["mars"].tick_seconds == Fraction(1, 1000)
            with pytest.raises(NousError) as failure:
                await runtime.configuration.update({"time.domains.mars.tick_seconds": "1/2"})
            assert failure.value.code == "configuration.binding_conflict"

            with pytest.raises(RuntimeError):
                async with runtime.database.write() as tx:
                    await register_domain(
                        TimeDomainDefinition(domain_id="rolled-back", kind="opaque", name="temp"),
                        tx,
                    )
                    raise RuntimeError("abort the business operation")
            assert "rolled-back" not in await runtime.time.domains()

            before_restart = runtime.configuration.snapshot
            result = await runtime.configuration.update(
                {"execution.max_background_tasks": 3, "logging.level": "ERROR"}
            )
            assert result["state"] == "saved_pending_restart"
            assert runtime.configuration.snapshot is before_restart
        async with open_instance(instance) as reopened:
            assert reopened.configuration.snapshot.values.execution.max_background_tasks == 3
            assert reopened.configuration.snapshot.values.logging.level == "ERROR"
            assert (await reopened.time.domains())["mars"].tick_seconds == Fraction(1, 1000)
            result = await reopened.configuration.update(
                {
                    "providers.gateway": {
                        "protocol": "openai-chat-completions",
                        "endpoint": "http://127.0.0.1:3000/v1",
                    },
                    "models.embedder": {
                        "provider": "gateway",
                        "model_name": "example-embedding",
                        "capabilities": ["embedding"],
                    },
                    "embedding.spaces.example": {
                        "model": "embedder",
                        "deployment_revision": "one",
                        "dimensions": 384,
                        "modalities": ["text"],
                        "normalization": "l2",
                        "encoding": {
                            "document_instruction": "",
                            "query_instruction": "",
                            "preprocessing_revision": "one",
                            "input_combination": "text",
                        },
                    },
                }
            )
            assert result["state"] == "saved_pending_restart"
        async with open_instance(instance) as reopened:
            result = await reopened.configuration.update(
                {"embedding.spaces.example.dimensions": 256}
            )
            assert result["state"] == "applied"
            definition = resolved_space(reopened.configuration.snapshot, "example")
            source = await reopened.artifacts.ingest("A document for the embedding space.")
            buffer = io.BytesIO()
            np.save(buffer, np.full((1, 256), 1 / 16, dtype=np.float32), allow_pickle=False)
            prepared = await reopened.results.prepare_outputs(
                {
                    "vectors": OutputContent(
                        data=buffer.getvalue(),
                        media_type="application/x-npy",
                        display_name="vectors.npy",
                    )
                }
            )
            async with reopened.database.write() as tx:
                await reopened.embedding_spaces.bind_in(tx, definition)
                adopted = await reopened.results.adopt_in(
                    tx,
                    facts=ResultFacts(
                        operation="embedding",
                        producer=ProgramProducer(operation="embedding"),
                        selected_inputs=SelectedInputs(
                            sources=(
                                SelectedSource(
                                    slot_key="S1",
                                    selections=(ContentSelection(artifact=source.ref),),
                                ),
                            )
                        ),
                        inputs=(
                            InputGroup(
                                key="S1",
                                slot_key="S1",
                                name="source",
                                parts=(InputPart(content=ContentSelection(artifact=source.ref)),),
                            ),
                        ),
                        formed_at=reopened.artifacts.clock.utc_now(),
                    ),
                    prepared_outputs=prepared,
                )
                await reopened.embedding_spaces.attach_in(
                    tx,
                    definition=definition,
                    result_id=adopted.ref.id,
                    artifact_id=adopted.outputs[0].artifact.id,
                    purpose="document",
                    input_digests=[
                        hashlib.sha256(b"A document for the embedding space.").hexdigest()
                    ],
                )
            for patch in (
                {"embedding.spaces.example.dimensions": 128},
                {"models.embedder.model_name": "another-deployment"},
            ):
                with pytest.raises(NousError) as conflict:
                    await reopened.configuration.update(patch)
                assert conflict.value.code == "configuration.binding_conflict"
            assert (await reopened.results.get(adopted.ref)).inputs[0].parts[
                0
            ].content.artifact == source.ref

    asyncio.run(scenario())
