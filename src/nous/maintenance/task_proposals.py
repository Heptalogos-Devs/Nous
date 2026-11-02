"""Turn small task results into domain proposals using the accepted source catalog."""

from __future__ import annotations

from nous_sdk.contracts import (
    DomainError,
    EpisodeBody,
    EpistemicStance,
    MemoryBody,
    SchemaBody,
    TimeRange,
)
from nous_sdk.contracts.growth import GrowthProposal, GrowthProposalAdoption, GrowthSnapshot
from nous_sdk.contracts.growth_bodies import (
    GrowthEvidenceRef,
    GrowthJournal,
    GrowthJournalParagraphSource,
    GrowthRef,
)
from nous_sdk.contracts.maintenance import MaintenanceProposal, MaintenanceRun
from nous_sdk.contracts.maintenance_tasks import DreamDraft, JournalDraft, ReflectionDraft
from pydantic import JsonValue, TypeAdapter


def document(
    run: MaintenanceRun, snapshot: GrowthSnapshot, value: JsonValue, call_id: str
) -> MaintenanceProposal:
    if run.command.kind == "journal":
        output = JournalDraft.model_validate(value)
        keys = {entry.key: entry for entry in snapshot.entries}
        available = set(keys) | {source.key for source in snapshot.sources}
        used = tuple(
            dict.fromkeys(key for paragraph in output.paragraphs for key in paragraph.source_keys)
        )
        if set(used) - available:
            raise DomainError(
                "journal_source_unknown",
                "Journal paragraphs must cite selected local sources",
                keys=sorted(set(used) - available),
            )
        period = run.command.period
        if period is None:
            episodes = [
                entry.record.body
                for entry in snapshot.entries
                if isinstance(entry.record.body, EpisodeBody)
            ]
            ranges = [body.experience for body in episodes if isinstance(body, EpisodeBody)]
            if ranges and len({item.domain for item in ranges}) == 1:
                period = TimeRange(
                    domain=ranges[0].domain,
                    start=min(
                        (item.start for item in ranges if item.start is not None), default=None
                    ),
                    end=max((item.end for item in ranges if item.end is not None), default=None),
                )
            else:
                points = [
                    point
                    for point in [
                        *(entry.record.times.observation for entry in snapshot.entries),
                        *(source.times.observation for source in snapshot.sources),
                    ]
                    if point is not None
                ]
                period = TimeRange(
                    domain=points[0].domain if points else "unix_ms",
                    start=min((point.coordinate for point in points), default=None),
                    end=max((point.coordinate + 1 for point in points), default=None),
                )
        revisits = {reference.exact() for reference in run.command.revisits}
        body = GrowthJournal(
            kind="journal",
            title=output.title,
            narrative="\n\n".join(paragraph.text for paragraph in output.paragraphs),
            period=period,
            perspective=EpistemicStance(classification="inferred"),
            episodes=tuple(
                GrowthRef(key=entry.key)
                for entry in snapshot.entries
                if entry.record.reference.kind == "episode"
            ),
            lineage=tuple(
                GrowthRef(key=entry.key)
                for entry in snapshot.entries
                if entry.record.reference.exact() in revisits
            ),
            paragraph_sources=tuple(
                GrowthJournalParagraphSource(
                    paragraph=index,
                    evidence=tuple(GrowthEvidenceRef(key=key) for key in paragraph.source_keys),
                )
                for index, paragraph in enumerate(output.paragraphs)
            ),
        )
        proposal = GrowthProposal(
            summary=output.title,
            rationale=run.command.instruction or "Journal composed from the selected experiences",
            actions=(
                GrowthProposalAdoption(
                    action="adopt",
                    key="journal",
                    body=body,
                    evidence_keys=used,
                    reason=run.command.instruction or "Record this period from its actual sources",
                ),
            ),
        )
        return MaintenanceProposal(
            summary=proposal.summary,
            rationale=proposal.rationale,
            actions=tuple(
                TypeAdapter(JsonValue).validate_python(action.model_dump(mode="json"))
                for action in proposal.actions
            ),
            source_call_id=call_id,
        )
    keys = tuple(entry.key for entry in snapshot.entries) + tuple(
        source.key for source in snapshot.sources
    )
    known = {
        entry.record.body.text.strip()
        for entry in snapshot.entries
        if isinstance(entry.record.body, MemoryBody)
    }
    actions: list[JsonValue] = []
    from nous_sdk.contracts.growth_bodies import GrowthMemory, GrowthSchema

    if run.command.kind in {"reflect", "growth"}:
        thought = ReflectionDraft.model_validate(value)
        for index, text in enumerate(thought.candidates):
            if text.strip() in known or not keys:
                continue
            body = GrowthMemory(
                kind="memory", text=text, epistemic=EpistemicStance(classification="inferred")
            )
            action = GrowthProposalAdoption(
                action="adopt",
                key=f"reflection{index}",
                body=body,
                evidence_keys=keys,
                reason="本次局部反思明确提出的新认识",
            )
            actions.append(TypeAdapter(JsonValue).validate_python(action.model_dump(mode="json")))
        return MaintenanceProposal(
            summary=thought.explanation,
            rationale="",
            actions=tuple(actions),
            questions=thought.questions,
            source_call_id=call_id,
        )
    thought = DreamDraft.model_validate(value)
    cognitive_keys = tuple(entry.key for entry in snapshot.entries)
    existing = {
        entry.record.body.claim.strip()
        for entry in snapshot.entries
        if isinstance(entry.record.body, SchemaBody)
    }
    for index, text in enumerate(thought.hypotheses):
        if not cognitive_keys or text.strip() in existing:
            continue
        body = GrowthSchema(
            kind="schema",
            title=text.splitlines()[0][:120],
            claim=text,
            scope=run.command.instruction,
            boundary="由当前所选资料提出，等待进一步资料核对。",
            support=tuple(GrowthRef(key=key) for key in cognitive_keys),
            epistemic=EpistemicStance(classification="inferred"),
        )
        action = GrowthProposalAdoption(
            action="adopt",
            key=f"hypothesis{index}",
            body=body,
            evidence_keys=keys,
            reason="本次有界联想提出的假设",
        )
        actions.append(TypeAdapter(JsonValue).validate_python(action.model_dump(mode="json")))
    return MaintenanceProposal(
        summary="\n\n".join(thought.hypotheses) or "本次未形成新假设。",
        rationale="",
        actions=tuple(actions),
        questions=thought.questions,
        source_call_id=call_id,
    )
