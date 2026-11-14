"""Explicit query controls preserve natural text and select real material groups."""

import asyncio
from pathlib import Path

import pytest

from nous.cognition.retrieval.contracts import (
    DeliveryRequest,
    QueryAll,
    QueryAny,
    QueryAtom,
    QueryRequest,
)
from nous.cognition.retrieval.nousql import parse_nousql
from nous.cognition.retrieval.operations import QUERY
from nous.cognition.writing.contracts import AddNote, AfterWrite, CreateJournal
from nous.cognition.writing.operations import ADD_NOTE, CREATE_JOURNAL
from nous.common.errors import NousError
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave.contracts import DeliveryBudget


def test_nousql_literals_nested_controls_and_invalid_syntax() -> None:
    literal = parse_nousql(r"Keep \$5, \@name, \#tag, and \\ with ordinary (x && y).")
    assert isinstance(literal, QueryAtom)
    assert literal.text == "Keep $5, @name, #tag, and \\ with ordinary (x && y)."
    expression = parse_nousql('$any("alpha", $all("beta", "gamma")) $limit(2) $return(note)')
    assert isinstance(expression, QueryAny)
    assert expression.controls.limit == 2 and expression.controls.return_types == ("note",)
    assert isinstance(expression.children[1], QueryAll)
    for source in ('$any("alpha", "beta"', "hello\x00world"):
        with pytest.raises(NousError):
            parse_nousql(source)


def test_nested_nousql_and_budget_through_application(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            alpha = await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text="alpha cedar."), remember=True, after=policy),
            )
            await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text="beta cobalt."), remember=True, after=policy),
            )
            both = await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text="beta gamma."), remember=True, after=policy),
            )
            await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(body=InlineText(text="alpha journal."), after=policy),
            )
            text = '$any("alpha", $all("beta", "gamma")) $return(note) $limit(2)'
            expected = {
                alpha.commit.revision_ref.model_dump_json(),
                both.commit.revision_ref.model_dump_json(),
            }
            result = await runtime.app.call(QUERY, QueryRequest(text=text))
            assert {
                item.material.ref.model_dump_json() for item in result.evidence.materials
            } == expected
            assert len(result.evidence.passages) == 2
            bounded = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text=text,
                    delivery=DeliveryRequest(budget=DeliveryBudget(amount=1, unit="bytes")),
                ),
            )
            assert {
                item.material.ref.model_dump_json() for item in bounded.evidence.materials
            } == expected
            assert not bounded.evidence.passages
            assert all(
                item.delivery_status == "budget_exhausted" for item in bounded.evidence.materials
            )
            assert {
                item.target.model_dump_json()
                for item in bounded.evidence.continuation
                if item.relation == "read"
            } == expected

    asyncio.run(scenario())
