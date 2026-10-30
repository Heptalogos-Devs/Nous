"""Fact identity and conflict groups follow A-Memorix's normalized-claim semantics."""

import unicodedata

from nous_sdk.contracts.fact_body import FactClaimSpec, FactScope


def normalized(value: str) -> str:
    return " ".join(unicodedata.normalize("NFKC", value).strip().split()).casefold()


def scope_identity(scope: FactScope) -> tuple[str, str]:
    return scope.kind, scope.identity


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
