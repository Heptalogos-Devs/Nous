"""Current retrieval preserves addressable text without inventing independent witnesses."""

import asyncio
from pathlib import Path

from nous.cognition.retrieval import CognitiveQuery
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_inspiration_only_journal_remains_searchable_without_fact_roots(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            note = await runtime.records.record_note(
                "default", "A comparison that inspired an idea."
            )
            inspiration = await runtime.records.select_sources(
                "default", [note.revision_ref], usage="inspiration"
            )
            journal = await runtime.records.create_journal(
                "default", "Speculation about resource ownership.", sources=inspiration
            )
            result = await runtime.cognitive_retrieval.query(
                CognitiveQuery(namespace="default", text="ownership", mode="lexical")
            )
            assert result["hits"][0]["revision"] == journal.revision_ref.address
            assert result["hits"][0]["sources"] == []
            assert result["scope"]["source_roots"] == 0
            assert (await runtime.records.read(journal.revision_ref)).sources == inspiration

    asyncio.run(scenario())


def test_source_projection_keeps_visible_labels_and_literal_markdown() -> None:
    from nous.content.citations import project_citations

    literal = "`[来源 S1](source:S1:C1)`\n\n```md\n[来源 S1](source:S1:C1)\n```\n"
    ordinary = "[普通链接](https://example.com/source:S1)"
    escaped = r"\[来源 S1](source:S1:C1)"
    original = (
        "正文[个人网站作者自述](source:S1:C1)。\n\n"
        "旧标记[来源 S1:C1](source:S1:C1)与[来源 S1](source:S1:C1)。\n\n"
        "[S1:C2的真实说明](source:S1:C1)\n\n" + literal + "\n" + ordinary + "\n\n" + escaped
    )
    expected = (
        "正文个人网站作者自述。\n\n旧标记来源 S1:C1与来源 S1。\n\nS1:C2的真实说明\n\n"
        + literal
        + "\n"
        + ordinary
        + "\n\n"
        + escaped
    )
    assert project_citations(original) == expected
    assert (
        project_citations("[资料 S1](source:S1:C1)[材料](source:S1:C1)[原文](source:S1:C1)")
        == "资料 S1材料原文"
    )
    assert (
        project_citations("[source S1](source:S1:C1) [자료](source:S1:C1) [原資料](source:S1:C1)")
        == "source S1 자료 原資料"
    )
    assert project_citations("<source:S1:C1>") == "source:S1:C1"
    assert project_citations("[个人网站原文](source:S1:C1)") == "个人网站原文"
    assert project_citations("[AT&amp;T](source:S1:C1)") == "AT&T"
    assert project_citations(r"[a\*b](source:S1:C1)") == "a*b"
    assert project_citations("[**text** and *more*](source:S1:C1)") == "text and more"
    assert project_citations("[see `a*b`](source:S1:C1)") == "see a*b"
    assert project_citations("> [甲\n> 乙](source:S1)\n") == "> 甲\n乙\n"
    assert project_citations("> [甲](\n> source:S1)\n") == "> 甲\n"


def test_retrieval_projection_preserves_original_annotation_and_plain_inputs(
    tmp_path: Path,
) -> None:
    from nous.cognition.contracts import SourceBinding
    from nous.content.annotations import AnnotationProposal
    from nous.content.contracts import ContentSelection
    from nous.content.projections import semantic_text

    async def scenario() -> None:
        instance = tmp_path / "projection"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            note = await runtime.records.record_note("default", "线程结束后关闭存储。")
            raw_note = await runtime.records.read(note.revision_ref)
            source = SourceBinding(
                key="S1:C1",
                content_selection=ContentSelection(artifact=raw_note.body_artifact),
                owner_revision=note.revision_ref,
            )
            body = "资源释放应等待线程结束。[个人网站作者自述](source:S1:C1)"
            journal = await runtime.records.create_journal("default", body, sources=(source,))
            raw = await runtime.records.read(journal.revision_ref)
            chosen = ContentSelection(artifact=raw.body_artifact)
            projected = await semantic_text(chosen, runtime.results)
            assert (await semantic_text(chosen, runtime.results)).content == projected.content
            assert projected.text == "资源释放应等待线程结束。个人网站作者自述"
            provenance = (
                await runtime.results.provenance((projected.content.artifact,), details=True)
            )[projected.content.artifact.id]
            assert any(relation.parent == raw.body_artifact for relation in provenance.relations)
            proposal = AnnotationProposal.model_validate(
                {
                    "concepts": [
                        {
                            "key": "K",
                            "label": "资源释放",
                            "description": "等待工作线程结束后释放其资源",
                        }
                    ],
                    "occurrences": [{"key": "O", "concept": "K", "blocks": ["B1"]}],
                    "paths": [],
                }
            )
            await runtime.annotations.annotate(journal.revision_ref, proposal=proposal)
            await runtime.records.create_journal("default", "资源释放需要明确的持有者。")
            query = await runtime.cognitive_retrieval.query(
                CognitiveQuery(namespace="default", text="资源释放", mode="lexical")
            )
            assert [hit["rank"] for hit in query["hits"]] == list(range(1, len(query["hits"]) + 1))
            hit = next(
                hit for hit in query["hits"] if hit["revision"] == journal.revision_ref.address
            )
            assert hit["content"]["artifact"]["id"] == raw.body_artifact.id
            assert hit["concepts"]
            assert (await runtime.records.read(journal.revision_ref)).body == body
            plain = await runtime.artifacts.ingest("普通文字")
            assert (
                await semantic_text(ContentSelection(artifact=plain.ref), runtime.results)
            ).content.artifact == plain.ref
            only = await runtime.records.create_journal(
                "default", "[](source:S1:C1)", sources=(source,)
            )
            query = await runtime.cognitive_retrieval.query(
                CognitiveQuery(namespace="default", text="C1", mode="lexical")
            )
            assert not query["hits"]
            assert (await runtime.records.read(only.revision_ref)).body == "[](source:S1:C1)"

    asyncio.run(scenario())
