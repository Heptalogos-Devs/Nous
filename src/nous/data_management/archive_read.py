"""Decode a Nous Namespace bundle archive."""

from io import BytesIO
from zipfile import BadZipFile, ZipFile

from nous_sdk.contracts import DomainError, NamespaceBundle


def decode(data: bytes) -> NamespaceBundle:
    try:
        with ZipFile(BytesIO(data)) as archive:
            info = archive.getinfo("bundle.json")
            if info.file_size > 64 * 1024 * 1024:
                raise DomainError("bundle_too_large", "Bundle JSON exceeds 64 MiB")
            return NamespaceBundle.model_validate_json(archive.read(info))
    except (BadZipFile, KeyError) as error:
        raise DomainError("bundle_format", "Archive does not contain a supported bundle") from error
