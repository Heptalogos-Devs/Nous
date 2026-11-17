"""Translate frozen Wave inputs into stable Native component identities and bundles."""

from __future__ import annotations

import hashlib
from dataclasses import fields
from typing import TYPE_CHECKING, cast

import numpy as np
import numpy.typing as npt

from .assets import FrozenInputs
from .contracts import (
    ResolvedBuildProfile,
)
from .serialization import (
    encode_json,
)

if TYPE_CHECKING:
    import nous_native


def component_signature(frozen: FrozenInputs, profile: ResolvedBuildProfile) -> str:
    """Hash every Native input, including its stable candidate-to-row layout."""
    digest = hashlib.sha256(encode_json(profile.model_dump(mode="json")).encode())
    for field in fields(frozen):
        if field.name in {"preparation_stats", "mapping", "vector_mappings", "candidate_data"}:
            continue
        value = getattr(frozen, field.name)
        digest.update(field.name.encode())
        if isinstance(value, np.ndarray):
            array = cast(npt.NDArray[np.generic], value)
            digest.update(encode_json({"dtype": array.dtype.str, "shape": array.shape}).encode())
            digest.update(array.tobytes(order="C"))
        else:
            digest.update(encode_json(value).encode())
    return digest.hexdigest()


def bundles(
    frozen: FrozenInputs,
) -> tuple[
    nous_native.Candidates,
    nous_native.Concepts,
    nous_native.SourceFacts,
    nous_native.Associations,
]:
    import nous_native as native

    return (
        native.Candidates(
            encode_json(frozen.candidate_metadata),
            frozen.candidate_texts,
            frozen.candidate_vectors,
            frozen.membership_indptr,
            frozen.membership_indices,
            frozen.path_indptr,
            frozen.path_step_indptr,
            frozen.path_indices,
            frozen.path_positions,
            frozen.candidate_root_indptr,
            frozen.candidate_root_indices,
        ),
        native.Concepts(encode_json(frozen.concept_metadata), frozen.concept_vectors),
        native.SourceFacts(
            encode_json(frozen.facts_metadata),
            frozen.sequence_roots,
            frozen.sequence_indptr,
            frozen.sequence_indices,
            frozen.sequence_positions,
            frozen.sequence_ordered,
        ),
        native.Associations(
            encode_json(frozen.association_metadata),
            frozen.association_sources,
            frozen.association_targets,
            frozen.association_directed,
            frozen.association_strengths,
            frozen.association_confidences,
            frozen.association_polarities,
            frozen.association_edge_groups,
            frozen.group_root_indptr,
            frozen.group_root_indices,
        ),
    )
