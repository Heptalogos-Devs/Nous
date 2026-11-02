"""Published generation reuse, immediate lexical deltas and partial dense coverage."""

import asyncio
import json
from pathlib import Path
from uuid import uuid4

import numpy as np
from nous_native import IndexStore, NativeError
from nous_sdk.contracts import CallContext, EmbeddingBatch, ImportDocument, QueryRequest

from nous.cognition.records import CognitiveRecords
from nous.cognition.schema import vectors
from nous.identity.service import IdentityService
from nous.material.service import MaterialService
from nous.platform.configuration.namespace_policy import entries
from nous.platform.configuration.service import ConfigurationEntry, ConfigurationService
from nous.platform.persistence import Authority
from nous.retrieval.policy import defaults
from nous.retrieval.profiles import WavePolicy
from nous.retrieval.service import RetrievalService


class QueryModels:
    async def embed(self, ctx, texts, *, source_ids=()):
        return EmbeddingBatch(
            model="test", space="test-space", dimensions=2, vectors=((1.0, 0.0),), call_ids=(None,)
        )


def test_published_generations_and_partial_semantic_coverage(tmp_path: Path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        cognition = CognitiveRecords(authority)
        materials = MaterialService(authority)
        configuration = ConfigurationService(
            authority,
            {
                **entries(),
                "retrieval": ConfigurationEntry(defaults()),
                "wave": ConfigurationEntry(WavePolicy()),
            },
            instance=tmp_path,
        )
        retrieval = RetrievalService(
            authority,
            cognition,
            QueryModels(),
            tmp_path / "indexes",
            materials=materials,
            configuration=configuration,
        )
        await authority.initialize()
        ctx = CallContext(caller_id="owner", namespace_id=str(uuid4()), credential_source_ids=None)
        try:
            await IdentityService(authority).create_namespace(ctx, "wave", "namespace")

            async def admit(text: str, operation: str):
                async with authority.write(
                    ctx, operation, operation_kind="material.admit", payload={"text": text}
                ) as tx:
                    admitted = await materials.admit_text_in(
                        tx, ctx, ImportDocument(text=text, filename=operation + ".txt")
                    )
                    tx.result(admitted.material["id"])
                return admitted

            first = await admit("alpha semantic", "first")
            # Admission is searchable before the asynchronous native publication.
            pending = await retrieval.query(
                ctx, QueryRequest(text="alpha", mode="lexical", domains=("media",))
            )
            assert pending.hits and pending.coverage.indexed_through == 0
            await retrieval.rebuild_namespace(ctx.namespace_id, 0)
            initial = await retrieval.query(
                ctx, QueryRequest(text="alpha", mode="lexical", domains=("media",))
            )
            assert initial.coverage.complete
            second = await admit("alpha lexical", "second")
            delta = await retrieval.query(
                ctx, QueryRequest(text="alpha", mode="lexical", domains=("media",))
            )
            assert len(delta.hits) == 2 and not delta.coverage.complete
            # Adopt an existing representation vector; the other material has none.
            reference = next(
                hit.object.reference.exact()
                for hit in delta.hits
                if hit.object.material_id == first.material["id"]
            )
            async with authority.write(
                ctx, "vector", operation_kind="test.vector", payload={"reference": reference}
            ) as tx:
                await tx.connection.execute(
                    vectors.insert().values(
                        reference=reference,
                        space="test-space",
                        model="test",
                        dimensions=2,
                        vector=[1.0, 0.0],
                        call_id=None,
                        commit_seq=tx.commit_seq,
                    )
                )
                tx.result(reference)
            await retrieval.rebuild_namespace(ctx.namespace_id, 0)
            before = {path.name for path in (tmp_path / "indexes").rglob("g-*")}
            hybrid = await retrieval.query(
                ctx, QueryRequest(text="alpha", mode="hybrid", domains=("media",))
            )
            assert len(hybrid.hits) == 2
            assert hybrid.execution.effective_mode == "hybrid"
            assert hybrid.execution.missing_vectors == 1
            assert hybrid.plan["native"]["dense"]["mode"] == "dense"
            assert {path.name for path in (tmp_path / "indexes").rglob("g-*")} == before
            from nous_sdk.contracts import QueryContext

            saved_cue = next(
                hit.object.reference
                for hit in hybrid.hits
                if hit.object.material_id == first.material["id"]
            )
            similar = await retrieval.query(
                ctx,
                QueryRequest(
                    mode="dense",
                    domains=("media",),
                    context=QueryContext(attachments=(saved_cue,)),
                ),
            )
            assert similar.hits[0].object.reference == saved_cue
            # QueryModels has no media embedding provider; this uses the saved cue.
            await retrieval.close()
            restarted = await retrieval.query(
                ctx, QueryRequest(text="alpha", mode="hybrid", domains=("media",))
            )
            assert restarted.coverage.generation == hybrid.coverage.generation
            denied = await retrieval.query(
                ctx.model_copy(update={"allowed_source_ids": ()}),
                QueryRequest(text="alpha", mode="lexical", domains=("media",)),
            )
            assert not denied.hits
            assert second.material["id"] != first.material["id"]
        finally:
            await retrieval.close()
            await authority.close()

    asyncio.run(scenario())


def test_vcp_stage_switches_control_native_execution(tmp_path: Path):
    store = IndexStore(str(tmp_path / "native"))
    try:
        snapshot = store.build(
            references=["memory:a@1", "memory:b@1"],
            texts=["alpha", "beta"],
            assets_json=json.dumps(
                {
                    "space": "stages",
                    "tags": [
                        {"key": "concept:a@1", "label": "alpha"},
                        {"key": "concept:b@1", "label": "beta"},
                    ],
                }
            ),
            vectors=np.asarray([[1, 0], [0.8, 0.2]], dtype=np.float32),
            tag_vectors=np.asarray([[1, 0], [0, 1]], dtype=np.float32),
            tag_indptr=np.asarray([0, 1, 2], dtype=np.uint64),
            tag_indices=np.asarray([0, 1], dtype=np.uint64),
        )
        result = snapshot.query(
            mode="vcp",
            text="alpha",
            vectors=np.asarray([1, 0], dtype=np.float32),
            allowed_rows=np.asarray([0, 1], dtype=np.uint64),
            options_json=json.dumps(
                {
                    "vcp": {
                        "pipeline": {
                            "epaEnabled": False,
                            "gatingEnabled": False,
                            "topologyEnabled": False,
                            "fieldsEnabled": False,
                        },
                        "include_fields": True,
                    }
                }
            ),
        )
        execution = json.loads(result.diagnostics_json)["vcp"]
        stages = execution["pipeline"]["diagnostics"]["stages"]
        assert stages == {
            "epa": "disabled",
            "gating": "disabled",
            "topology": "disabled",
            "fields": "disabled",
            "pyramid": "completed",
        }
        assert execution["pipeline"]["diagnostics"]["dualField"]["iterations"] == 0
        assert execution["fields"]["local"] == []
        assert execution["fields"]["observation"]["edges"] == []
    finally:
        store.close()


def test_native_assets_merge_cooccurrence_with_directed_relations(tmp_path: Path):
    store = IndexStore(str(tmp_path / "native"))
    try:
        snapshot = store.build(
            references=["memory:a@1", "memory:b@1"],
            texts=["alpha", "beta"],
            assets_json=json.dumps(
                {
                    "space": "cooccurrence",
                    "tags": [
                        {"key": "concept:a@1", "label": "alpha"},
                        {"key": "concept:b@1", "label": "beta"},
                    ],
                    "artifact_config": {"usage_gain": {"concept:a@1": 0.5}},
                }
            ),
            vectors=np.asarray([[1, 0], [0.8, 0.2]], dtype=np.float32),
            tag_vectors=np.asarray([[1, 0], [0.8, 0.2]], dtype=np.float32),
            tag_indptr=np.asarray([0, 2, 4], dtype=np.uint64),
            tag_indices=np.asarray([0, 1, 0, 1], dtype=np.uint64),
            source_references=["actual-material", "other-material"],
            candidate_source_indptr=np.asarray([0, 1, 2], dtype=np.uint64),
            candidate_source_indices=np.asarray([0, 1], dtype=np.uint64),
            graph_indptr=np.asarray([0, 1, 1], dtype=np.uint64),
            graph_indices=np.asarray([1], dtype=np.uint64),
            graph_weights=np.asarray([1.0], dtype=np.float64),
            graph_source_indptr=np.asarray([0, 1], dtype=np.uint64),
            graph_source_indices=np.asarray([0], dtype=np.uint64),
        )
        result = snapshot.query(
            mode="vcp",
            vectors=np.asarray([1, 0], dtype=np.float32),
            allowed_rows=np.asarray([0, 1], dtype=np.uint64),
        )
        assets = json.loads(result.diagnostics_json)["vcp"]["assets"]
        assert assets["graph"]["graph_source"] == "cooccurrence+association"
        assert assets["graph"]["association_edges"] == 1
        assert assets["graph"]["cooccurrence_edges"] == 2
        assert assets["graph"]["usage_nodes"] == 1
        assert assets["source_root_count"] == 2
        restricted = snapshot.query(
            mode="vcp",
            vectors=np.asarray([1, 0], dtype=np.float32),
            allowed_rows=np.asarray([0], dtype=np.uint64),
        )
        diagnostics = json.loads(restricted.diagnostics_json)["vcp"]
        assert diagnostics["scoped_stage_availability"]["geometry"] == "unavailable_in_scope"
        assert diagnostics["assets"]["source_root_count"] == 1
        assert restricted.generation == snapshot.generation
        try:
            snapshot.query(
                mode="vcp",
                vectors=np.asarray([1, 0], dtype=np.float32),
                allowed_rows=np.asarray([0], dtype=np.uint64),
                options_json='{"strict_capabilities":true}',
            )
        except NativeError as error:
            assert error.code == "capability_unavailable"
        else:
            raise AssertionError("Strict geometry cannot use unauthorized source contributions")
    finally:
        store.close()


def test_query_context_retains_query_and_scoped_session_work(tmp_path: Path):
    async def scenario():
        from nous_sdk.contracts import (
            AdoptCognition,
            DomainError,
            EntityBody,
            EvidenceLink,
            QueryContext,
        )
        from nous_sdk.contracts.context import (
            ChangeContextSession,
            ChangeWorkContext,
            WorkContextContent,
        )
        from nous_sdk.contracts.targets import CognitiveTarget

        from nous.application import Application
        from nous.platform.configuration import Settings

        app = Application(Settings(instance=tmp_path))
        ctx = CallContext(caller_id="host", namespace_id=str(uuid4()), credential_source_ids=None)
        await app.authority.initialize()
        try:
            await app.identity.create_namespace(ctx, "query-context", "create")
            async with app.authority.write(
                ctx, "source", operation_kind="material.admit", payload={}
            ) as tx:
                admitted = await app.material.admit_text_in(
                    tx,
                    ctx,
                    ImportDocument(text="Alice and Bob are known people.", filename="people.txt"),
                )
                tx.result(admitted.material["id"])
            people = []
            for label in ("Alice", "Bob"):
                people.append(
                    await app.cognition.adopt(
                        ctx,
                        AdoptCognition(
                            operation_id=label,
                            body=EntityBody(label=label, entity_type="person"),
                            evidence=(EvidenceLink(observation_id=admitted.event["id"]),),
                        ),
                    )
                )
            work = await app.contexts.change_work(
                ctx,
                ChangeWorkContext(
                    operation_id="work",
                    action="create",
                    content=WorkContextContent(
                        purpose="Find Alice's decisions",
                        anchors=(CognitiveTarget(reference=people[0].reference),),
                    ),
                ),
            )
            session = await app.contexts.open_session(ctx, "session")
            await app.contexts.change_session(
                ctx,
                ChangeContextSession(
                    operation_id="select-work",
                    session_id=session.id,
                    expected_revision=session.revision,
                    active_work_context_id=work.id,
                ),
            )
            result = await app.retrieval.query(
                ctx, QueryRequest(text="Alice", mode="lexical", context="session:" + session.id)
            )
            assert {hit.object.reference.exact() for hit in result.hits} == {
                people[0].reference.exact()
            }
            assert result.plan["query_context"]["original_query"] == "Alice"
            assert result.plan["query_context"]["goal"] == "Find Alice's decisions"
            try:
                await app.retrieval.query(
                    ctx, QueryRequest(text="Alice $mode(dense)", mode="lexical")
                )
            except DomainError as error:
                assert error.code == "query_option_conflict"
            else:
                raise AssertionError("Conflicting primary query options require one choice")
            natural = "她之前怎么说的 / what did they say / それは"
            broad = await app.retrieval.query(
                ctx,
                QueryRequest(
                    text=natural, mode="lexical", context=QueryContext(entities=("Alice", "Bob"))
                ),
            )
            assert broad.plan["query_context"]["original_query"] == natural
            assert broad.plan["query_context"]["entities"] == ["Alice", "Bob"]
            try:
                await app.retrieval.query(
                    ctx.model_copy(update={"caller_id": "another-host"}),
                    QueryRequest(text="Alice", mode="lexical", context="session:" + session.id),
                )
            except DomainError as error:
                assert error.code == "session_not_found"
            else:
                raise AssertionError("Session handles retain their host's scope")
        finally:
            await app.close()

    asyncio.run(scenario())
