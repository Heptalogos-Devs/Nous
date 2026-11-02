"""Claim content, applicability and conflicts owned by the Fact domain."""

import unicodedata

from nous_sdk.contracts import TimeRange
from nous.platform.errors import DomainError
from nous_sdk.contracts.fact_body import FactClaimSpec, FactScope
from nous_sdk.contracts.facts import FactChange

type ClaimIdentity = tuple[
    tuple[str, str, str, str, str], tuple[str, int | None, int | None] | None
]


def claim_identity(claim: FactClaimSpec, validity: TimeRange | None) -> ClaimIdentity:
    return (identity(claim), (validity.domain, validity.start, validity.end) if validity else None)


def requested_validity(command: FactChange, previous: TimeRange | None) -> TimeRange | None:
    if "validity" in command.model_fields_set:
        if command.validity_patch is not None:
            raise DomainError(
                "fact_validity_ambiguous", "Choose a full validity range or a partial update"
            )
        return command.validity
    patch = command.validity_patch
    if patch is None:
        return previous
    if previous is not None and previous.domain != patch.domain:
        raise DomainError(
            "fact_validity_domain", "Partial validity updates must keep the original clock domain"
        )
    return TimeRange(
        domain=patch.domain,
        start=patch.start
        if "start" in patch.model_fields_set
        else previous.start
        if previous
        else None,
        end=patch.end if "end" in patch.model_fields_set else previous.end if previous else None,
    )


def conflicts_with(
    left: FactClaimSpec,
    left_validity: TimeRange | None,
    right: FactClaimSpec,
    right_validity: TimeRange | None,
) -> bool:
    if identity(left) == identity(right) or conflict_group(left) != conflict_group(right):
        return False
    for validity in (left_validity, right_validity):
        if validity is not None and validity.start is not None and (validity.start == validity.end):
            return False
    if left_validity is None or right_validity is None:
        return True
    if left_validity.domain != right_validity.domain:
        return False
    return not (
        left_validity.end is not None
        and right_validity.start is not None
        and (left_validity.end <= right_validity.start)
        or (
            right_validity.end is not None
            and left_validity.start is not None
            and (right_validity.end <= left_validity.start)
        )
    )


def normalized(value: str) -> str:
    return " ".join(unicodedata.normalize("NFKC", value).strip().split()).casefold()


def scope_identity(scope: FactScope) -> tuple[str, str]:
    return (scope.kind, scope.identity)


def identity(claim: FactClaimSpec) -> tuple[str, str, str, str, str]:
    return (
        *scope_identity(claim.scope),
        normalized(claim.fact_key),
        normalized(claim.value_text),
        claim.polarity,
    )


def conflict_group(claim: FactClaimSpec) -> tuple[str, str, str, str | None]:
    return (
        *scope_identity(claim.scope),
        normalized(claim.fact_key),
        normalized(claim.value_text) if claim.cardinality == "set" else None,
    )
