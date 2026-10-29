"""Decode supported archives through their format's own validation rules."""

from io import BytesIO
from zipfile import BadZipFile, ZipFile

from nous_wave.contracts import DomainError, SubjectBundle


def decode(data: bytes) -> SubjectBundle:
    from .portable_adoption import normalize
    from .portable_read import load

    try:
        with ZipFile(BytesIO(data)) as archive:
            if "manifest.json" in archive.namelist():
                portable = load(data)
                if "bundle.json" in portable.members:
                    return SubjectBundle.model_validate_json(portable.members["bundle.json"])
                return normalize(portable, data)
            info = archive.getinfo("bundle.json")
            if info.file_size > 64 * 1024 * 1024:
                raise DomainError("bundle_too_large", "Bundle JSON exceeds 64 MiB")
            return SubjectBundle.model_validate_json(archive.read(info))
    except (BadZipFile, KeyError) as error:
        raise DomainError("bundle_format", "Archive does not contain a supported bundle") from error
