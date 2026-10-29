"""Typed parameters accepted by the native query algorithms.

Values are required here; the server obtains defaults from native Serde schemas.
"""

from typing import Literal

from pydantic import ConfigDict, Field, FiniteFloat, model_validator
from pydantic.alias_generators import to_camel

from .common import Contract


class NativeParameters(Contract):
    model_config = ConfigDict(
        extra="forbid",
        frozen=True,
        alias_generator=to_camel,
        populate_by_name=True,
        serialize_by_alias=True,
    )


class VcpSpikeRouting(NativeParameters):
    base_decay: FiniteFloat
    base_momentum: FiniteFloat
    dynamic_wormhole_max_targets: int = Field(ge=0)
    dynamic_wormhole_slots: int = Field(ge=0)
    dynamic_wormhole_threshold: FiniteFloat
    dynamic_wormholes: bool
    fir_gamma: FiniteFloat
    firing_threshold: FiniteFloat
    max_neighbors_per_node: int = Field(ge=0)
    max_output_edges: int = Field(ge=0)
    max_output_nodes: int = Field(ge=0)
    max_propagation_states: int = Field(ge=0)
    max_safe_hops: int = Field(ge=0)
    max_transition_records: int = Field(ge=0)
    minimum_injected_current: FiniteFloat
    return_flow_factor: FiniteFloat
    tension_threshold: FiniteFloat
    wormhole_decay: FiniteFloat


class VcpPipeline(NativeParameters):
    activation_multiplier: tuple[FiniteFloat, FiniteFloat]
    base_tag_boost: FiniteFloat
    core_boost_factor: FiniteFloat
    core_boost_range: tuple[FiniteFloat, FiniteFloat]
    deduplication_threshold: FiniteFloat
    dynamic_boost_range: tuple[FiniteFloat, FiniteFloat]
    epa_gate_mode: Literal["absolute", "relativeLinear", "relativeSqrt", "backgroundRelative"]
    field_max_iterations: int = Field(ge=0)
    lang_confidence_enabled: bool
    lang_penalty_cross_domain: FiniteFloat
    lang_penalty_unknown: FiniteFloat
    layer_decay: FiniteFloat
    local_alpha: FiniteFloat
    local_mass_ratio: FiniteFloat
    local_tolerance: FiniteFloat
    max_emergent_nodes: int = Field(ge=0)
    max_field_projection_nodes: int = Field(ge=0)
    max_fusion_tags: int = Field(ge=0)
    max_levels: int = Field(ge=0)
    min_energy_ratio: FiniteFloat
    normal_tag_threshold: FiniteFloat
    pyramid_top_k: int = Field(ge=0)
    spike_routing: VcpSpikeRouting
    tech_tag_threshold: FiniteFloat
    transfer_alpha: FiniteFloat
    transfer_mass_ratio: FiniteFloat
    transfer_tolerance: FiniteFloat

    @model_validator(mode="after")
    def ordered_ranges(self) -> "VcpPipeline":
        for lower, upper in (
            self.activation_multiplier,
            self.dynamic_boost_range,
            self.core_boost_range,
        ):
            if lower > upper:
                raise ValueError("VCP range endpoints must be ordered")
        return self


class VcpReadout(NativeParameters):
    anchor_activation_floor: FiniteFloat
    anchor_activation_z: FiniteFloat
    anchor_bonus_cap: FiniteFloat
    anchor_frontier_abs_floor: FiniteFloat
    anchor_frontier_contrast: FiniteFloat
    anchor_k: int = Field(ge=0)
    anchor_saturation: FiniteFloat
    bm25_k: int = Field(alias="bm25K", ge=0)
    closure_floor: FiniteFloat
    collapsed_threshold: FiniteFloat
    conditional_bandwidth: FiniteFloat
    conditional_closure_bandwidth: FiniteFloat
    conditional_direct_bandwidth: FiniteFloat
    denoised_k: int = Field(ge=0)
    direction_floor: FiniteFloat
    fallback_reliability_cap: FiniteFloat
    innovation_confidence_z: FiniteFloat
    innovation_scale: FiniteFloat
    kappa_edge: FiniteFloat
    kappa_ratio: FiniteFloat
    local_field_k: int = Field(ge=0)
    local_weight: FiniteFloat
    max_union_candidates: int = Field(ge=0)
    maximum_river_edges: int = Field(ge=0)
    minimum_effective_peers: FiniteFloat
    minimum_peers: int = Field(ge=0)
    minimum_river_edge_flow: FiniteFloat
    node_only_reliability_cap: FiniteFloat
    omega_epsilon: FiniteFloat
    omega_gamma: FiniteFloat
    pure_local_weight: FiniteFloat
    pure_query_weight: FiniteFloat
    pure_transfer_weight: FiniteFloat
    query_k: int = Field(ge=0)
    rarity_floor: FiniteFloat
    relative_distance_temperature: FiniteFloat
    reliability_seed_saturation: FiniteFloat
    reverse_direction_credit: FiniteFloat
    semantic_anchor_discount: FiniteFloat
    semantic_anchor_threshold: FiniteFloat
    semantic_node_threshold: FiniteFloat
    sparse_threshold: FiniteFloat
    specificity_floor: FiniteFloat
    struct_role_min_omega: FiniteFloat
    topology_bonus_cap: FiniteFloat
    topology_path_saturation: FiniteFloat
    transfer_field_k: int = Field(ge=0)
    transfer_weight: FiniteFloat


class VcpPolicy(Contract):
    pipeline: VcpPipeline
    readout: VcpReadout
    candidate_limit: int = Field(ge=1)
    core_tags: tuple[str, ...]
    include_trace: bool
    include_fields: bool
    thinking: bool


class RetrievalPolicy(Contract):
    lexical_weight: FiniteFloat = Field(ge=0)
    dense_weight: FiniteFloat = Field(ge=0)
    rrf_k: FiniteFloat = Field(gt=0)
    vcp: VcpPolicy
