"""Host image bytes enter the same original upload and contact APIs as other media."""

import base64
from email.encoders import encode_noop
from email.mime.image import MIMEImage
from hashlib import md5, sha256
from typing import TYPE_CHECKING

from pydantic import JsonValue

from ..types import JsonObject

if TYPE_CHECKING:
    from .client import AMemorixAdapter


async def image_memory(
    adapter: "AMemorixAdapter",
    *,
    action: str,
    image_bytes: bytes | None = None,
    **kwargs: JsonValue,
) -> JsonObject:
    values = dict(kwargs)
    encoded = values.pop("image_base64", None)
    if encoded is not None:
        if image_bytes is not None or not isinstance(encoded, str):
            raise ValueError("Supply exactly one image bytes or base64 input")
        image_bytes = base64.b64decode(encoded, validate=True)
    if action in {"describe", "observe"}:
        values.setdefault("source_kind", "model_description" if action == "describe" else "manual")
    if image_bytes is not None:
        if action not in {"ingest", "search"}:
            raise ValueError("Image bytes are accepted for ingest or search")
        external = values.get("external_ref")
        if action == "ingest" and (not isinstance(external, str) or not external):
            raise ValueError("Image ingest requires the actual external_ref")
        media_type = MIMEImage(image_bytes, _encoder=encode_noop).get_content_type()
        digest = sha256(image_bytes).hexdigest()
        legacy = md5(image_bytes, usedforsecurity=False).hexdigest()
        operation = (
            values.get("operation_id")
            or "image:" + sha256((str(external or "query") + ":" + digest).encode()).hexdigest()
        )
        if not isinstance(operation, str):
            raise ValueError("operation_id must be text")

        async def chunks():
            yield image_bytes

        receipt = await adapter.client.upload_stream(
            adapter.namespace_id,
            chunks(),
            byte_len=len(image_bytes),
            
            media_type=media_type,
            source=f"amemorix-image:{legacy}",
            expected_content_hash=digest,
            operation_id="image-original:" + digest,
        )
        values.update(material_id=receipt.result.id, content_hash=digest, operation_id=operation)
    return await adapter.call("image_memory", {"action": action, **values})
