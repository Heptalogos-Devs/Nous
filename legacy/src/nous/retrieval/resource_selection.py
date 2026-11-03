"""Resolve explicit source-object names and fetch only selected registered resources."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import QueryRequest
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.references import ResourceTarget
from nous_sdk.contracts.resources import ReadResource, ResourceRecord

from .planning import BoundPlan, Cue

if TYPE_CHECKING:
    from .service import RetrievalService


def source_domain(locator: str) -> str | None:
    prefix = locator.split(":", 1)[0]
    return prefix if prefix in {"resource", "observation", "evidence"} else None


async def prepare_sources(
    self: RetrievalService, ctx: CallContext, bound: QueryRequest, plan: BoundPlan
) -> QueryRequest:
    domains: list[str] = list(bound.domains)
    if bound.reference and source_domain(bound.reference):
        locator = bound.reference
        domain = source_domain(locator)
        if domain is not None:
            domains = [domain]
        if domain != "resource" and "@" not in locator:
            locator += "@1"
        plan["selectors"].append({"kind": "selector", "selector": "ref", "locators": [locator]})
        bound = bound.model_copy(update={"reference": None})
    resource_names: list[str] = []
    resource_locators: list[str] = []
    selectors: list[Cue] = []
    for cue in plan["selectors"]:
        if cue.get("selector") == "resource":
            media_names: list[str] = []
            for name in cue.get("locators", []):
                if name.startswith(
                    (
                        "material:",
                        "source_region:",
                        "source-region:",
                        "representation:",
                        "derived-region:",
                        "derived_region:",
                    )
                ):
                    media_names.append(name)
                else:
                    resource_names.append(name)
            if media_names:
                selectors.append({**cue, "locators": media_names})
            continue
        if cue.get("selector") == "ref":
            locators: list[str] = []
            for locator in cue.get("locators", []):
                domain = source_domain(locator)
                if domain and domain not in domains:
                    domains.append(domain)
                if domain == "resource":
                    resource_locators.append(locator)
                else:
                    locators.append(locator + "@1" if domain and "@" not in locator else locator)
            if locators:
                selectors.append({**cue, "locators": locators})
        else:
            selectors.append(cue)
    plan["selectors"] = selectors
    external = (
        bound.current != "none"
        or bound.filters.authority in {"external_current_authority", "resource_descriptor"}
        or resource_names
        or resource_locators
    )
    if not external:
        return QueryRequest.model_validate({**bound.model_dump(), "domains": tuple(domains)})
    if self.resources is None:
        raise DomainError(
            "resource_directory_unavailable", "External resource directory is unavailable"
        )
    if bound.filters.authority == "external_current_authority":
        bound = bound.model_copy(
            update={
                "current": "required",
                "filters": bound.filters.model_copy(update={"authority": None}),
            }
        )
    available: list[ResourceRecord] = []
    cursor: str | None = None
    while True:
        page = await self.resources.list(ctx, cursor=cursor, limit=1000)
        available.extend(record for record in page.items if record.enabled)
        cursor = page.next_cursor
        if cursor is None:
            break
    selected: dict[str, ResourceRecord] = {}
    for locator in resource_locators:
        resolved = await self.references.resolve(ctx, locator, bound.as_of_commit)
        if not isinstance(resolved.reference, ResourceTarget):
            raise DomainError("resource_required", "Select an external resource")
        reference = resolved.reference.reference
        record = await self.resources.get(ctx, reference.id, reference.revision)
        selected[record.reference.id] = record
    for name in resource_names:
        resolved = await self.references.resolve(
            ctx, name if name.startswith("resource:") else "resource:" + name, bound.as_of_commit
        )
        if not isinstance(resolved.reference, ResourceTarget):
            raise DomainError("resource_required", "Select an external resource")
        reference = resolved.reference.reference
        record = await self.resources.get(ctx, reference.id, reference.revision)
        selected[reference.id] = record
    if not selected and bound.current != "none":
        matches = [
            record for record in available if record.name.casefold() in bound.text.casefold()
        ]
        if not matches and len(available) == 1:
            matches = available
        if not matches and available:
            raise DomainError(
                "resource_selector_required",
                "Select a registered resource by name",
                candidates=[
                    {"name": record.name, "reference": record.reference.model_dump(mode="json")}
                    for record in available
                ],
            )
        selected = {record.reference.id: record for record in matches}
    if selected:
        records = list(selected.values())
        maximum = min(bound.limit, bound.budget.candidates)
        plan["resource_refresh_complete"] = len(records) <= maximum
        checked: list[str] = []
        locators = []
        for record in records[:maximum]:
            if bound.current != "none":
                record = await self.resources.read(
                    ctx,
                    ReadResource(
                        operation_id="query-resource:" + str(uuid4()),
                        resource_id=record.reference.id,
                        current=bound.current,
                    ),
                    automatic=True,
                )
                if record.error_code is None:
                    checked.append(record.reference.exact())
            locators.append(record.reference.exact())
        plan["checked_resources"] = checked
        plan["selectors"].append({"kind": "selector", "selector": "ref", "locators": locators})
    elif bound.current != "none":
        plan["no_matches"] = True
    return QueryRequest.model_validate({**bound.model_dump(), "domains": ("resource",)})
