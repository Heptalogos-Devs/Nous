"""Compose the frozen catalog, adoption and durable admission owners."""

from __future__ import annotations

from nous.cognition.models import ModelServices
from nous.cognition.service import CognitionService
from nous.material.service import MaterialService
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority

from . import (
    admission,
    attempts,
    catalog,
    embedding,
    file_completion,
    observation,
    observation_jobs,
    processing,
    replay,
    status,
    text_admission,
)


class ImportService:
    submit_text_import = text_admission.submit_text_import
    read_text_import = text_admission.read_text_import
    run_text_import = text_admission.run_text_import
    replay_catalog_in = replay.replay_catalog_in
    replay_catalog = replay.replay_catalog
    replay_result_in = replay.replay_result_in
    replay_result = replay.replay_result
    read_admit_result = replay.read_admit_result
    input_context_in = replay.input_context_in
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
