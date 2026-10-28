"""Compose the frozen catalog, adoption and durable admission owners."""

from __future__ import annotations

from nous.cognition.models import ModelServices
from nous.cognition.service import CognitionService
from nous.material.service import MaterialService
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority

from . import admission, attempts, catalog, embedding, processing


class ImportService:
    prepare_document = catalog.prepare_document
    freeze_catalog = catalog.freeze_catalog
    freeze_catalog_prepared = catalog.freeze_catalog_prepared
    freeze_catalog_in = catalog.freeze_catalog_in
    read_catalog_in = catalog.read_catalog_in
    read_catalog = catalog.read_catalog
    submit_process = processing.submit_process
    run_process = processing.run_process
    process_catalog = processing.process_catalog
    read_attempts = attempts.read_attempts
    complete_files = processing.complete_files
    embed_output = embedding.embed_output
    eligible_file_in = admission.eligible_file_in
    accept_admit = admission.accept_admit
    finish_admit = admission.finish_admit
    admit_files = admission.admit_files
    submit_admit_files = admission.submit_admit_files

    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        models: ModelServices,
        content_store: ContentStore | None = None,
        materials: MaterialService | None = None,
    ) -> None:
        self.authority = authority
        self.cognition = cognition
        self.models = models
        self.materials = materials or MaterialService(authority, content_store)
        self.content_store = self.materials.content_store
