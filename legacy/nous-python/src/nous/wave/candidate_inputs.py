"""Prepare candidate selections and title-aware semantic text."""

import hashlib
import json
from collections.abc import AsyncIterator, Sequence
from typing import cast

from nous.application.preparation import PreparedText, TextPreparation
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    OutputContent,
    OutputMapping,
    ProgramProducer,
    ResultFacts,
    SelectedInputs,
    SelectedSource,
    SourceSelection,
)
from nous.content.results import PreparedOutputs

from .contracts import Candidate, CandidateContent, CorpusView, Document, DocumentCandidates


async def prepare_candidates(
    manifest: CorpusView,
    documents: dict[str, Document],
    accepted: dict[str, ArtifactRef],
    preparation: TextPreparation,
    cached: dict[str, list[dict[str, object]]],
    layouts: dict[str, list[dict[str, object]]],
) -> AsyncIterator[tuple[Candidate | None, PreparedText | None, dict[str, object] | None]]:
    """Normalize exact inputs and document layouts through one content preparation session."""
    await preparation.prepare_many(
        tuple(
            ContentSelection(
                artifact=accepted[documents[declaration.content.document].key],
                selection=documents[declaration.content.document].input.selector.restrict(
                    declaration.content.selector
                ),
            )
            for declaration in manifest.candidates
            if declaration.document not in cached and declaration.document not in layouts
        )
    )
    for declaration in manifest.candidates:
        if declaration.document in cached:
            for item in cached[declaration.document]:
                key = cast(str, cast(dict[str, object], item["mapping"])["key"])
                if (
                    isinstance(declaration, Candidate)
                    and key == declaration.key
                    or isinstance(declaration, DocumentCandidates)
                    and (
                        key == f"{declaration.group_key}:whole"
                        or key.startswith(f"{declaration.group_key}:part:")
                    )
                ):
                    yield None, None, item
            continue
        if declaration.document in layouts:
            assert isinstance(declaration, DocumentCandidates)
            for item in layouts[declaration.document]:
                mapping = cast(dict[str, object], item["mapping"])
                local = cast(bool, mapping["local"])
                old_key = cast(str, mapping["key"])
                key = (
                    f"{declaration.group_key}:part:{old_key.rsplit(':part:', 1)[1]}"
                    if local
                    else f"{declaration.group_key}:whole"
                )
                selection = ContentSelection.model_validate(mapping["content"]).selection
                yield (
                    Candidate(
                        key=key,
                        document=declaration.document,
                        group_key=declaration.group_key,
                        content=CandidateContent(
                            document=declaration.document,
                            selector=selection if local else SourceSelection(),
                        ),
                        concepts=declaration.concepts,
                        occurrences=declaration.occurrences,
                        paths=declaration.paths,
                        annotation_ref=declaration.annotation_ref,
                        annotation_concepts=declaration.annotation_concepts,
                    ),
                    None,
                    item,
                )
            continue
        document = documents[declaration.content.document]
        origin = ContentSelection(
            artifact=accepted[document.key],
            selection=document.input.selector.restrict(declaration.content.selector),
        )
        prepared = await preparation.prepare(origin)
        if isinstance(declaration, Candidate):
            yield declaration, prepared, None
            continue
        assert isinstance(declaration, DocumentCandidates)

        def exact(
            key: str, selector: SourceSelection, declaration: DocumentCandidates = declaration
        ) -> Candidate:
            return Candidate(
                key=key,
                document=declaration.document,
                group_key=declaration.group_key,
                content=CandidateContent(document=declaration.document, selector=selector),
                concepts=declaration.concepts,
                occurrences=declaration.occurrences,
                paths=declaration.paths,
                annotation_ref=declaration.annotation_ref,
                annotation_concepts=declaration.annotation_concepts,
            )

        if declaration.whole:
            yield exact(f"{declaration.group_key}:whole", SourceSelection()), prepared, None
        if declaration.local_layout is not None:
            chunks = await preparation.artifacts.content.execution.blocking(
                prepared.view.complete_chunks, declaration.local_layout.target_bytes
            )
            for index, chunk in enumerate(chunks, 1):
                if declaration.whole and chunk.text == prepared.view.text:
                    continue
                selector = chunk.selection.selection
                yield (
                    exact(f"{declaration.group_key}:part:{index}", selector),
                    await preparation.artifacts.content.execution.blocking(
                        prepared.slice, selector
                    ),
                    None,
                )


async def covers(
    content: ContentSelection, selections: Sequence[SourceSelection], preparation: TextPreparation
) -> bool:
    normalized = await preparation.artifacts.normalize_selections(
        (
            content,
            *(ContentSelection(artifact=content.artifact, selection=part) for part in selections),
        )
    )
    low, high = normalized[0].selection.start_byte, normalized[0].selection.end_byte
    assert low is not None and high is not None
    for value in normalized[1:]:
        start, end = value.selection.start_byte, value.selection.end_byte
        assert start is not None and end is not None
        if start < low or end > high:
            return False
    return True


async def titled_semantics(
    preparation: TextPreparation,
    contents: Sequence[ContentSelection],
    texts: Sequence[str],
    titles: Sequence[str],
) -> tuple[ContentSelection, ...]:
    """Adopt title-plus-body inputs, mapping only their body back to source text."""
    requests: dict[str, tuple[ContentSelection, str, str]] = {}
    order: list[str | None] = []
    for content, text, title in zip(contents, texts, titles, strict=True):
        if not title:
            order.append(None)
            continue
        key = hashlib.sha256(
            json.dumps(
                {"content": content.model_dump(mode="json"), "title": title},
                ensure_ascii=False,
                sort_keys=True,
            ).encode("utf-8")
        ).hexdigest()
        requests[key] = (content, text, title)
        order.append(key)
    results = preparation.preparer.results
    async with results.database.read() as tx:
        adopted = await results.cached_outputs_in(
            tx, "wave.titled_semantic_text", tuple(requests), output="text"
        )
    pending = [key for key in requests if key not in adopted]
    for start in range(0, len(pending), 100):
        keys = pending[start : start + 100]
        sources = await preparation.artifacts.normalize_selections(
            [requests[key][0] for key in keys]
        )
        artifacts = await preparation.artifacts.get_many([source.artifact for source in sources])
        outputs: list[PreparedOutputs] = []
        for key in keys:
            _, text, title = requests[key]
            outputs.append(
                await results.prepare_outputs(
                    {
                        "text": OutputContent(
                            data=f"{title}\n\n{text}".encode(),
                            media_type="text/plain",
                            display_name="retrieval-text.txt",
                        )
                    }
                )
            )
        async with results.database.write() as tx:
            adopted.update(
                await results.cached_outputs_in(
                    tx, "wave.titled_semantic_text", keys, output="text"
                )
            )
            for key, source, artifact, output in zip(
                keys, sources, artifacts, outputs, strict=True
            ):
                if key in adopted:
                    continue
                _, text, title = requests[key]
                offset = len(f"{title}\n\n".encode())
                result = await results.adopt_in(
                    tx,
                    facts=ResultFacts(
                        operation="wave.titled_semantic_text",
                        producer=ProgramProducer(operation="wave.titled_semantic_text"),
                        selected_inputs=SelectedInputs(
                            sources=(SelectedSource(slot_key="body", selections=(source,)),)
                        ),
                        inputs=(
                            InputGroup(
                                key="body",
                                slot_key="body",
                                name="Body",
                                parts=(
                                    InputPart(
                                        content=source,
                                        representation="semantic_text",
                                        media_type=artifact.media_type,
                                    ),
                                ),
                            ),
                        ),
                        parameters={"cache_key": key, "title": title, "title_field": "title"},
                        mappings=(
                            OutputMapping(
                                output_name="text",
                                relation="identity",
                                origins=(source,),
                                selection=SourceSelection(
                                    start_byte=offset, end_byte=offset + len(text.encode("utf-8"))
                                ),
                            ),
                        ),
                        formed_at=preparation.artifacts.clock.utc_now(),
                    ),
                    prepared_outputs=output,
                )
                adopted[key] = (
                    result.ref,
                    next(value.artifact for value in result.outputs if value.name == "text"),
                )
    return tuple(
        content if key is None else ContentSelection(artifact=adopted[key][1])
        for content, key in zip(contents, order, strict=True)
    )
