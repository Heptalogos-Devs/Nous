"""Own editor files until the written revision has been confirmed to the user."""

import hashlib
from pathlib import Path
from uuid import uuid4

from pydantic import BaseModel, ConfigDict

from nous.cognition.contracts import RecordRef, RecordRevisionRef, RecordView
from nous.cognition.writing.contracts import AfterWrite
from nous.common.errors import NousError
from nous.common.files import write_text
from nous.runtime.paths import ResolvedPaths


class EditSession(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    id: str
    target: RecordRef
    expected: RecordRevisionRef
    original_digest: str
    file: Path
    after: AfterWrite

    @property
    def descriptor(self) -> Path:
        return self.file.with_suffix(".json")

    @classmethod
    def create(cls, paths: ResolvedPaths, original: RecordView, after: AfterWrite) -> EditSession:
        session_id = str(uuid4())
        file = paths.resolve_owned("data", Path("recovery") / "edits" / f"{session_id}.md")
        session = cls(
            id=session_id,
            target=original.ref,
            expected=original.revision_ref,
            original_digest=hashlib.sha256(original.body.encode("utf-8")).hexdigest(),
            file=file,
            after=after,
        )
        # Publish the recovery identity before the editor is allowed to modify the file.
        write_text(session.descriptor, session.model_dump_json(indent=2))
        write_text(file, original.body)
        return session

    @classmethod
    def resume(cls, descriptor: Path, data: bytes, instance_id: str, kind: str) -> EditSession:
        try:
            session = cls.model_validate_json(data)
        except ValueError as error:
            raise NousError(
                "input.edit_session_invalid",
                "Cannot read the edit recovery session.",
                details={"session": str(descriptor)},
                cause=error,
            ) from error
        if (
            session.target.instance_id != instance_id
            or session.target.kind != kind
            or session.expected.record_ref != session.target
            or descriptor.stem != session.id
        ):
            raise NousError(
                "input.edit_session_mismatch",
                "The edit recovery session belongs to another instance or target.",
                details={"session": str(descriptor)},
            )
        return session.model_copy(update={"file": descriptor.with_suffix(".md")})

    def unchanged(self, body: str) -> bool:
        return hashlib.sha256(body.encode("utf-8")).hexdigest() == self.original_digest

    def clear(self) -> None:
        self.descriptor.unlink(missing_ok=True)
        self.file.unlink(missing_ok=True)
