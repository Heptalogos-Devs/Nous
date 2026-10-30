"""Public media operations retain exact original and derived identities."""

from collections.abc import Mapping
from pathlib import Path
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from ..contracts import MediaEmbeddingResult, MediaReference, SourceRegion
from ..contracts.context import MediaTarget
from ..contracts.derivation import (
    CreateDerivedRegion,
    CreateRepresentation,
    DerivedRegion,
    DerivedRepresentation,
    DeriveMedia,
    MediaLineage,
    RepresentationRef,
)
from ..operations import encoded, segment
from ..source_scope import SourceReadScope
from ..types import Download
from .bundles import BundleClient
from .requests import operation_identity, operation_values, request_scope, required_text


class MediaClient(BundleClient):
    async def embed_media(
        self,
        namespace_id: str,
        subject_id: str,
        reference: MediaReference,
        *,
        operation_id: str | None = None,
    ) -> MediaEmbeddingResult:
        return MediaEmbeddingResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/media/embed",
                body=encoded(reference),
                params={"operation_id": operation_id or str(uuid4())},
            )
        )

    async def derive_media(
        self, namespace_id: str, subject_id: str, command: DeriveMedia
    ) -> tuple[DerivedRepresentation, ...]:
        return TypeAdapter(tuple[DerivedRepresentation, ...]).validate_python(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/media/derive",
                body=encoded(command),
            )
        )

    async def adopt_representation(
        self, namespace_id: str, subject_id: str, command: CreateRepresentation
    ) -> DerivedRepresentation:
        return DerivedRepresentation.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/media/representations",
                body=encoded(command),
            )
        )

    async def representation(
        self, namespace_id: str, subject_id: str, reference: RepresentationRef
    ) -> DerivedRepresentation:
        return DerivedRepresentation.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/media/representations/read",
                body=encoded(reference),
            )
        )

    async def representation_history(
        self, namespace_id: str, subject_id: str, representation_id: str
    ) -> tuple[DerivedRepresentation, ...]:
        return TypeAdapter(tuple[DerivedRepresentation, ...]).validate_python(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/media/representations/{segment(representation_id)}/history",
            )
        )

    async def create_derived_region(
        self, namespace_id: str, subject_id: str, command: CreateDerivedRegion
    ) -> DerivedRegion:
        return DerivedRegion.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/media/derived-regions",
                body=encoded(command),
            )
        )

    async def derived_region(
        self, namespace_id: str, subject_id: str, region_id: str
    ) -> DerivedRegion:
        return DerivedRegion.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/media/derived-regions/{segment(region_id)}",
            )
        )

    async def create_source_region(
        self,
        namespace_id: str,
        subject_id: str,
        region: SourceRegion,
        *,
        operation_id: str | None = None,
    ) -> SourceRegion:
        return SourceRegion.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/media/source-regions",
                body={"operation_id": operation_id or str(uuid4()), "region": encoded(region)},
            )
        )

    async def source_region(
        self, namespace_id: str, subject_id: str, region_id: str
    ) -> SourceRegion:
        return SourceRegion.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + f"/regions/{segment(region_id)}",
            )
        )

    async def media_lineage(
        self, namespace_id: str, subject_id: str, reference: MediaReference
    ) -> MediaLineage:
        return MediaLineage.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/media/lineage",
                body=encoded(reference),
            )
        )

    async def download_derived(
        self,
        namespace_id: str,
        subject_id: str,
        reference: MediaReference,
        destination: str | Path,
    ) -> Download:
        if reference.kind not in {"representation", "derived_region"}:
            raise ValueError("Derived download requires an exact representation or derived region")
        lineage = await self.media_lineage(namespace_id, subject_id, reference)
        return await self.transport.download(
            lineage.root_material_ids[0],
            Path(destination),
            self._source_read,
            namespace_id=namespace_id,
            subject_id=subject_id,
            reference=reference,
        )

    async def media(self, request: Mapping[str, JsonValue]) -> JsonValue:
        values = operation_values(request)
        source_read = values.pop("source_read", None)
        if source_read is not None:
            return await self.with_source_read(SourceReadScope.model_validate(source_read)).media(
                {"action": request.get("action"), "request": values}
            )
        action = request.get("action")
        if action == "inspect":
            return encoded(await self.material(required_text(values, "material_id")))
        if action == "run":
            return encoded(
                await self.job(
                    required_text(values, "job_id"),
                    namespace_id=self.namespace_id,
                    subject_id=self.subject_id,
                )
            )
        namespace, subject = await request_scope(self, values)
        if action == "embed":
            return encoded(
                await self.embed_media(
                    namespace,
                    subject,
                    MediaReference.model_validate(values.pop("reference")),
                    operation_id=operation_identity(values),
                )
            )
        if action in {"source", "lineage"}:
            reference = values.pop("reference")
            if isinstance(reference, str):
                resolved = (await self.resolve_reference(subject, reference)).reference
                if not isinstance(resolved, MediaTarget):
                    raise ValueError("Media action requires a media reference")
                media = resolved.reference
            else:
                media = MediaReference.model_validate(reference)
            if action == "source":
                return await self.source(subject, media)
            return encoded(await self.media_lineage(namespace, subject, media))
        if action == "derive":
            values["operation_id"] = operation_identity(values)
            return [
                encoded(value)
                for value in await self.derive_media(
                    namespace, subject, DeriveMedia.model_validate(values)
                )
            ]
        if action == "adopt_representation":
            values["operation_id"] = operation_identity(values)
            return encoded(
                await self.adopt_representation(
                    namespace, subject, CreateRepresentation.model_validate(values)
                )
            )
        if action == "representation":
            return encoded(
                await self.representation(
                    namespace, subject, RepresentationRef.model_validate(values.pop("reference"))
                )
            )
        if action == "history":
            return [
                encoded(item)
                for item in await self.representation_history(
                    namespace, subject, required_text(values, "representation_id")
                )
            ]
        if action == "create_source_region":
            operation_id = operation_identity(values)
            region = SourceRegion.model_validate(values.pop("region", values))
            return encoded(
                await self.create_source_region(
                    namespace, subject, region, operation_id=operation_id
                )
            )
        if action == "source_region":
            return encoded(
                await self.source_region(namespace, subject, required_text(values, "id"))
            )
        if action == "create_derived_region":
            values["operation_id"] = operation_identity(values)
            if "parent" in values:
                values["parent_id"] = values.pop("parent")
            return encoded(
                await self.create_derived_region(
                    namespace, subject, CreateDerivedRegion.model_validate(values)
                )
            )
        if action == "derived_region":
            return encoded(
                await self.derived_region(namespace, subject, required_text(values, "id"))
            )
        raise ValueError("Unknown media action")
