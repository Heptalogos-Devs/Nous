"""Compose compilation, admission, session state and source purge behavior."""

from nous.cognition.service import CognitionService
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.models import ModelService
from nous.platform.persistence import Authority
from nous.retrieval.service import RetrievalService

from . import admission, compilation, purge, session_state
from .jobs import DurableContexts


class ManagedContexts(DurableContexts):
    authority: Authority
    cognition: CognitionService
    retrieval: RetrievalService | None
    materials: MaterialService | None
    models: ModelService | None
    derivations: DerivedService | None

    read_preparation_in = compilation.read_preparation_in
    read_track = compilation.read_track
    require_targets_in = compilation.require_targets_in
    media_segment_in = compilation.media_segment_in
    prepare = compilation.prepare
    record_segment = staticmethod(compilation.record_segment)
    preparation = compilation.preparation
    cancel_preparation = compilation.cancel_preparation
    admit_managed = admission.admit_managed
    report_managed = admission.report_managed
    read_admission = admission.read_admission
    purge_references_in = purge.purge_references_in
    open_session = session_state.open_session
    close_session = session_state.close_session
    change_session = session_state.change_session
