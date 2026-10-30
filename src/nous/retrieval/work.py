"""One request-wide work budget, shared by every composed query branch."""

from dataclasses import dataclass

from nous_sdk.contracts import DomainError
from nous_sdk.contracts.query_conditions import QueryBudget


@dataclass
class QueryWork:
    branches: int
    concepts: int
    text_bytes: int

    @classmethod
    def from_budget(cls, budget: QueryBudget) -> QueryWork:
        return cls(budget.branches, budget.vcp_concepts, budget.vcp_text_bytes)

    def branch(self) -> None:
        if self.branches < 1:
            raise DomainError("query_budget_exceeded", "Query exceeds branch budget")
        self.branches -= 1

    def cues(self, count: int, text_bytes: int) -> None:
        if count > self.concepts or text_bytes > self.text_bytes:
            raise DomainError(
                "query_budget_exceeded", "VCP intent exceeds the request-wide cue budget"
            )
        self.concepts -= count
        self.text_bytes -= text_bytes
