"""Compile named retrieval policies before any query work is admitted."""

from __future__ import annotations

from typing import Literal

from nous_sdk.contracts import CallContext, Contract, DomainError, QueryRequest
from pydantic import Field, model_validator

from nous.platform.configuration.service import ConfigurationService


class WaveProfile(Contract):
    channels: tuple[Literal["lexical", "dense", "entities"], ...] = ("lexical", "dense", "entities")
    expanders: tuple[Literal["vcp"], ...] = ()
    fusion: Literal["rrf", "vcp_readout"] = "rrf"
    reranker: Literal["compute", "cross_encoder", "llm", "off"] = "compute"
    capability_policy: Literal["strict", "best_effort"] = "best_effort"
    cross_encoder_model: str | None = None

    @model_validator(mode="after")
    def legal_stages(self) -> WaveProfile:
        if not self.channels:
            raise ValueError("A Wave profile needs at least one candidate channel")
        if self.expanders and ("dense" not in self.channels or self.fusion != "vcp_readout"):
            raise ValueError("VCP expansion requires dense candidates and vcp_readout fusion")
        if self.fusion == "vcp_readout" and "vcp" not in self.expanders:
            raise ValueError("vcp_readout fusion requires the VCP expander")
        return self


class AccretionPolicy(Contract):
    usage_half_life_days: float = Field(default=30, gt=0, allow_inf_nan=False)
    maximum_usage_gain: float = Field(default=1, ge=0, allow_inf_nan=False)


class WavePolicy(Contract):
    accretion: AccretionPolicy = Field(default_factory=AccretionPolicy)
    default_profile: str = "balanced"
    profiles: dict[str, WaveProfile] = Field(
        default_factory=lambda: {
            "balanced": WaveProfile(),
            "lexical": WaveProfile(channels=("lexical", "entities")),
            "associative": WaveProfile(expanders=("vcp",), fusion="vcp_readout"),
        }
    )

    @model_validator(mode="after")
    def known_default(self) -> WavePolicy:
        if self.default_profile not in self.profiles:
            raise ValueError("Wave default_profile must name a configured profile")
        return self


async def compile_profile(
    configuration: ConfigurationService,
    ctx: CallContext,
    request: QueryRequest,
) -> tuple[QueryRequest, dict[str, object]]:
    # Existing explicitly selected modes retain their public request semantics.
    if request.wave_profile is None and "mode" in request.model_fields_set:
        if request.capability_policy is not None:
            required = "required" if request.capability_policy == "strict" else "optional"
            request = request.model_copy(
                update={
                    "capabilities": request.capabilities.model_copy(
                        update={"semantic": required, "exploration": required}
                    )
                }
            )
        return request, {
            "profile": "explicit",
            "mode": request.mode,
            "reranker": request.reranker or "compute",
        }
    effective = await configuration.effective(ctx, "wave")
    policy = WavePolicy.model_validate(effective["value"])
    name = request.wave_profile or policy.default_profile
    profile = policy.profiles.get(name)
    if profile is None:
        raise DomainError("query_profile_not_found", "Unknown Wave profile", profile=name)
    strict = (request.capability_policy or profile.capability_policy) == "strict"
    mode = (
        "vcp"
        if profile.expanders
        else "hybrid"
        if "dense" in profile.channels and "lexical" in profile.channels
        else "dense"
        if "dense" in profile.channels
        else "lexical"
    )
    if "mode" in request.model_fields_set:
        mode = request.mode
    update: dict[str, object] = {
        "mode": mode,
        "reranker": request.reranker or ("llm" if request.rerank != "off" else profile.reranker),
        "capability_policy": "strict" if strict else "best_effort",
        "capabilities": request.capabilities.model_copy(
            update={
                "semantic": "required" if strict else "optional",
                "exploration": "required" if strict else "optional",
            }
        ),
    }
    return QueryRequest.model_validate(
        {**request.model_dump(mode="json", exclude_unset=True), **update}
    ), {
        "profile": name,
        "configuration_origin": effective["origin"],
        "channels": list(profile.channels),
        "expanders": list(profile.expanders),
        "fusion": profile.fusion,
        "reranker": update["reranker"],
        "capability_policy": update["capability_policy"],
        "cross_encoder_model": profile.cross_encoder_model,
    }
