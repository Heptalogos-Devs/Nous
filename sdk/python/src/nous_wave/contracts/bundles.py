"""Portable release selection and the receiving placement of an installation."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import Field, model_validator

from .common import Contract


class PackageDescriptor(Contract):
    id: str = Field(min_length=1)
    name: str = Field(min_length=1)
    version: str | None = None
    description: str = Field(default="", max_length=16384)
    author: str = Field(default="", max_length=1024)


class BundlePlacement(Contract):
    kind: Literal["global", "chat"] = "global"
    conversation_ref: str | None = None

    @model_validator(mode="after")
    def actual_placement(self) -> BundlePlacement:
        if (self.kind == "chat") != bool(self.conversation_ref):
            raise ValueError(
                "Chat placement requires its captured conversation_ref; global placement has none"
            )
        return self


class AllSelection(Contract):
    kind: Literal["all"] = "all"


class ChatSelection(Contract):
    kind: Literal["chat"] = "chat"
    conversation_ref: str = Field(min_length=1)


class SourceSelection(Contract):
    kind: Literal["source"] = "source"
    sources: tuple[str, ...] = Field(min_length=1)


class MaterialSelection(Contract):
    kind: Literal["materials"] = "materials"
    material_ids: tuple[str, ...] = Field(min_length=1)


class ImageSelection(Contract):
    kind: Literal["images"] = "images"
    content_hashes: tuple[str, ...] = ()
    occurrence_ids: tuple[str, ...] = ()


class PackageSelection(Contract):
    kind: Literal["package"] = "package"
    package_id: str | None = None
    installation_id: str | None = None

    @model_validator(mode="after")
    def one_package(self) -> PackageSelection:
        if (self.package_id is None) == (self.installation_id is None):
            raise ValueError("Select one package_id or installation_id")
        return self


class PackageTokenSelection(Contract):
    kind: Literal["package_token"] = "package_token"
    token: str = Field(min_length=1)


class ImportTaskSelection(Contract):
    kind: Literal["import_task"] = "import_task"
    job_id: str = Field(min_length=1)


BundleSelector = Annotated[
    AllSelection
    | ChatSelection
    | SourceSelection
    | MaterialSelection
    | ImageSelection
    | PackageSelection
    | PackageTokenSelection
    | ImportTaskSelection,
    Field(discriminator="kind"),
]


class BundleAssetOptions(Contract):
    include_vectors: bool = False
    include_images: bool = True
    include_image_related: bool = False
