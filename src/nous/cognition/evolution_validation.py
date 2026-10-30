"""Structural evolution invariants shared by review validation and actual writes."""

from collections.abc import Sequence

from nous_sdk.contracts import CognitiveRecord, ConceptBody, ConceptPartition, DomainError

from .growth_binding import GrowthValue


def validate_merge(records: Sequence[CognitiveRecord | GrowthValue]) -> None:
    if len({record.reference.id for record in records}) != len(records) or not all(
        isinstance(record.body, ConceptBody) and record.body.state == "active" for record in records
    ):
        raise DomainError("concept_merge", "Merge requires distinct active Concepts")


def validate_split(
    source: CognitiveRecord | GrowthValue, partitions: tuple[ConceptPartition, ...]
) -> None:
    if not isinstance(source.body, ConceptBody) or source.body.state != "active":
        raise DomainError("concept_split", "Split requires an active Concept")
    original = {reference.exact() for reference in source.body.members}
    members = [reference.exact() for partition in partitions for reference in partition.members]
    if set(members) != original or len(members) != len(set(members)):
        raise DomainError(
            "concept_partition", "Split partitions must preserve every original member exactly once"
        )
