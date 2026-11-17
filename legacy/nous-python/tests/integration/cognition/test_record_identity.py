"""Exact revisions and namespace identity survive changes to names and heads."""

import asyncio
from pathlib import Path

import pytest

from nous.cognition.contracts import CreateNamespace, RenameNamespace
from nous.cognition.scope import CREATE_NAMESPACE, RENAME_NAMESPACE
from nous.cognition.writing.contracts import (
    AddNote,
    AfterWrite,
    ReadRecord,
    RecordMembership,
    ReplaceRecord,
    SetRecall,
)
from nous.cognition.writing.operations import (
    ADD_NOTE,
    ARCHIVE_RECORD,
    READ_RECORD,
    REPLACE_RECORD,
    RESTORE_RECORD,
    SET_RECALL,
)
from nous.common.errors import NousError
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_renamed_namespace_and_cross_record_preconditions(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "identity"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.app.call(CREATE_NAMESPACE, CreateNamespace(name="research"))
            await runtime.app.call(
                RENAME_NAMESPACE, RenameNamespace(namespace=original.ref, name="science")
            )
            replacement = await runtime.app.call(CREATE_NAMESPACE, CreateNamespace(name="research"))
            assert replacement.ref != original.ref
            policy = AfterWrite(annotation="skip")
            first = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    namespace=original.ref,
                    body=InlineText(text="first"),
                    remember=True,
                    after=policy,
                ),
            )
            other = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    namespace=original.ref,
                    body=InlineText(text="other"),
                    remember=True,
                    after=policy,
                ),
            )
            checks = (
                runtime.app.call(
                    REPLACE_RECORD,
                    ReplaceRecord(
                        target=first.commit.ref,
                        expected=other.commit.revision_ref,
                        body=InlineText(text="changed"),
                        after=policy,
                    ),
                ),
                runtime.app.call(
                    ARCHIVE_RECORD,
                    RecordMembership(
                        target=first.commit.ref, expected=other.commit.revision_ref, after=policy
                    ),
                ),
                runtime.app.call(
                    RESTORE_RECORD,
                    RecordMembership(
                        target=first.commit.ref, expected=other.commit.revision_ref, after=policy
                    ),
                ),
                runtime.app.call(
                    SET_RECALL,
                    SetRecall(
                        target=first.commit.ref,
                        expected=other.commit.revision_ref,
                        enabled=False,
                        after=policy,
                    ),
                ),
            )
            for check in checks:
                with pytest.raises(NousError) as error:
                    await check
                assert error.value.code == "cognition.target_mismatch"
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=first.commit.ref))
            ).body == "first"
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=other.commit.ref))
            ).body == "other"
            with pytest.raises(NousError) as error:
                await runtime.app.call(
                    READ_RECORD, ReadRecord(target=first.commit.ref, namespace="research")
                )
            assert error.value.code == "cognition.namespace_mismatch"
            updated = await runtime.app.call(
                REPLACE_RECORD,
                ReplaceRecord(
                    target=first.commit.ref,
                    expected=1,
                    body=InlineText(text="changed"),
                    after=policy,
                ),
            )
            assert updated.commit.revision_ref.ordinal == 2
            assert updated.commit.namespace.ref == original.ref
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=first.commit.revision_ref))
            ).body == "first"

    asyncio.run(scenario())
