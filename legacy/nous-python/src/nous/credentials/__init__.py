"""Resolve explicitly configured instance credentials for outbound connections."""

import os
from collections.abc import Mapping
from pathlib import Path

from pydantic import SecretStr

from nous.common.errors import NousError
from nous.models.configuration import CredentialReference, EnvironmentCredential


class Credentials:
    """Resolve configured references at connection creation using the instance directory as root."""

    def __init__(
        self,
        instance_directory: Path,
        references: Mapping[str, CredentialReference],
        environment: Mapping[str, str] | None = None,
    ) -> None:
        self._directory = instance_directory
        self._references = references
        self._environment = os.environ if environment is None else environment

    def resolve(self, reference: str) -> SecretStr:
        """Resolve a configured credential reference for a connection owner.

        Relative credential file paths are rooted at the instance directory and
        file contents are stripped. Environment values are read as supplied.
        Missing references, unreadable files and empty values raise ``NousError``
        identifying the reference and location without returning the secret.
        """
        entry = self._references.get(reference)
        if entry is None:
            raise NousError(
                "credentials.reference_unknown",
                "The configured credential reference does not exist.",
                details={"reference": reference},
            )
        if isinstance(entry, EnvironmentCredential):
            value = self._environment.get(entry.name, "")
            location = entry.name
        else:
            path = Path(entry.path).expanduser()
            if not path.is_absolute():
                path = self._directory / path
            location = str(path)
            try:
                value = path.read_text(encoding="utf-8").strip()
            except OSError as error:
                raise NousError(
                    "credentials.unavailable",
                    "The configured credential file could not be read.",
                    details={"reference": reference, "location": location},
                    cause=error,
                ) from error
        if not value:
            raise NousError(
                "credentials.unavailable",
                "The configured credential is empty or missing.",
                details={"reference": reference, "location": location},
            )
        return SecretStr(value)
