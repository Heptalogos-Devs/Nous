"""Compose compilation, admission, session state and source purge behavior."""
from __future__ import annotations
from typing import TYPE_CHECKING
from nous.cognition.records import CognitiveRecords
from nous.cognition.motivation_records import MotivationService
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.models import ModelService
from nous.platform.persistence import Authority
from nous.retrieval.service import RetrievalService
from . import admission, compilation, model_call, purge, session_state, source_tools, tracking
from .jobs import DurableContexts
if TYPE_CHECKING:
    from nous.references.reading import ReadingService
    from nous.feedback.service import FeedbackService
    from nous.symbols.service import SymbolsService

class ManagedContexts(DurableContexts):
    authority: Authority
    cognition: CognitiveRecords
    reading: ReadingService
    motivation: MotivationService
    retrieval: RetrievalService
    materials: MaterialService
    models: ModelService
    derivations: DerivedService
    feedback_owner: FeedbackService
    symbols: SymbolsService
    read_preparation_in = compilation.read_preparation_in
    read_track = tracking.read_track
    require_targets_in = compilation.require_targets_in
    require_projection_in = compilation.require_projection_in
    stored_projection_in = compilation.stored_projection_in
    media_segment_in = compilation.media_segment_in
    prepare = compilation.prepare
    record_segment_in = compilation.record_segment_in
    preparation = compilation.preparation
    cancel_preparation = compilation.cancel_preparation
    admit_managed = admission.admit_managed
    report_managed = admission.report_managed
    read_admission = admission.read_admission
    preview_model = model_call.preview_model
    invoke_model = model_call.invoke_model
    read_context_source = source_tools.read_context_source
    purge_references_in = purge.purge_references_in
    open_session = session_state.open_session
    close_session = session_state.close_session
    change_session = session_state.change_session
