"""Bounded machine expressions compile into the one native NousQL grammar."""

from __future__ import annotations

import json

from nous_sdk.contracts import DomainError
from nous_sdk.contracts.query_conditions import QueryAtom, QueryBudget, QueryExpression


def source(expression: QueryExpression, budget: QueryBudget) -> str:
    leaves = 0

    def visit(node: QueryExpression, depth: int) -> str:
        nonlocal leaves
        if depth > budget.depth:
            raise DomainError("query_budget_exceeded", "Expression nesting exceeds query depth")
        if isinstance(node, QueryAtom):
            leaves += 1
            if leaves > budget.branches:
                raise DomainError("query_budget_exceeded", "Expression exceeds query branch budget")
            return node.source
        return (
            f"${node.operation}("
            + ",".join(
                json.dumps(visit(child, depth + 1), ensure_ascii=False) for child in node.children
            )
            + ")"
        )

    return visit(expression, 1)
