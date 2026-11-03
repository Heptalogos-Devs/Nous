"""Compose the frozen catalog, adoption and durable admission owners."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

from nous.cognition.formation import FormationService
from nous.cognition.models import ModelServices
from nous.cognition.records import CognitiveRecords
from nous.material.service import MaterialService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority

from . import (
    admission,
    attempts,
    catalog,
    contact,
    embedding,
    file_completion,
    observation,
    observation_jobs,
    processing,
    results,
    status,
    text_admission,
)

if TYPE_CHECKING:
    from nous.vector_projection.service import VectorProjectionService

    from .media import MediaImportService


class ImportService:
    jobs_for_observations = observation_jobs.observation_jobs
    contact = contact.contact
    submit_text_import = text_admission.submit_text_import
    read_text_import = text_admission.read_text_import
    run_text_import = text_admission.run_text_import
    read_admit_result = results.read_admit_result
    input_context_in = results.input_context_in
    vector_status = status.vector_status
    prepare_document = catalog.prepare_document
    freeze_catalog = catalog.freeze_catalog
    preview_text_import = catalog.preview_text_import
    freeze_catalog_prepared = catalog.freeze_catalog_prepared
    freeze_catalog_in = catalog.freeze_catalog_in
    read_catalog_in = catalog.read_catalog_in
    read_catalog = catalog.read_catalog
    submit_process = processing.submit_process
    run_process = processing.run_process
    process_catalog = processing.process_catalog
    read_attempts = attempts.read_attempts
    form_observation = observation.form_observation
    submit_form_observation = observation_jobs.submit_form_observation
    submit_form_observation_in = observation_jobs.submit_form_observation_in
    run_form_observation = observation_jobs.run_form_observation
    complete_files = file_completion.complete_files
    embed_output = embedding.embed_output
    embedding_sync = embedding.embedding_sync
    embed_records = embedding.embed_records
    eligible_file_in = admission.eligible_file_in
    accept_admit = admission.accept_admit
    finish_admit = admission.finish_admit
    admit_files = admission.admit_files
    submit_admit_files = admission.submit_admit_files

    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        models: ModelServices,
        content_store: ContentStore,
        materials: MaterialService,
        *,
        projection: VectorProjectionService,
        media_imports: MediaImportService,
        formation: FormationService,
        configuration: ConfigurationService,
    ) -> None:
        self.configuration = configuration
        self.formation = formation
        self.authority = authority
        self.cognition = cognition
        self.models = models
        self.materials = materials
        self.content_store = content_store
        self.projection = projection
        self.media_imports = media_imports

    async def read_unit_in(self, view, ctx, unit_id: str, *, catalog_id: str | None = None):
        from sqlalchemy import select
        from nous_sdk.contracts import ImportUnit, MediaReference
        from nous.platform.errors import DomainError
        from nous_sdk.contracts.derivation import TextRepresentation, StructuredRepresentation
        from nous.material.service import require_observation_in
        from nous.material.text import source_region_text
        from nous.platform.content import ContentObject
        from .schema import catalogs, units

        statement = (
            select(units.c.record)
            .join(catalogs, catalogs.c.id == units.c.catalog_id)
            .where(units.c.id == unit_id, catalogs.c.namespace_id == ctx.namespace_id)
        )
        if catalog_id is not None:
            statement = statement.where(units.c.catalog_id == catalog_id)
        raw = await view.connection.scalar(statement)
        if raw is None:
            raise DomainError("unit_not_found", "Import unit is unavailable in this Namespace")
        value = dict(raw)
        if value.get("observation_id"):
            await require_observation_in(
                view,
                ctx,
                value["observation_id"],
                active=not value.get("purged", False),
                region_id=value.get("source_region_id"),
            )
        if value.get("purged"):
            return ImportUnit.model_validate(
                {**value, "text": "", "context": {}, "split_context": {}}
            )
        reference = MediaReference.model_validate(value["source"])
        text = ""
        if reference.kind in {"representation", "derived_region"}:
            payload = (await self.cognition.derivations.selected_in(view, ctx, reference)).payload
            if (
                isinstance(payload, StructuredRepresentation)
                and payload.schema_id == "nous.import-unit/1"
            ):
                value.update(payload.value)
                return ImportUnit.model_validate(value)
            if isinstance(payload, TextRepresentation):
                text = payload.text
        elif value.get("strategy") != "media":
            material, region, _ = await self.materials.media_source_in(view, ctx, reference)
            text = material["text"]
            if text is None:
                async with self.content_store.protect(
                    ContentObject(
                        material["content_hash"], material["byte_len"], material["media_type"]
                    )
                ) as path:
                    text = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
            if region is not None:
                text = source_region_text(text, region)
        return ImportUnit.model_validate({**value, "text": text})
