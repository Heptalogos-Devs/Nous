"""Prompt restoration and one role budget across actual request purposes."""

import asyncio
from pathlib import Path

import pytest
from jinja2 import UndefinedError
from nous_wave.contracts import CallContext

from nous.platform.errors import NousError
from nous.platform.models.ledger import CallLedger
from nous.platform.persistence import Authority
from nous.platform.prompts import PromptService


def test_prompt_preview_history_restore_and_restart(tmp_path: Path):
    async def scenario():
        path = tmp_path / "authority.sqlite"
        authority = Authority(path)
        prompts = PromptService(authority)
        await authority.initialize()
        ctx = CallContext(caller_id="instance-admin", namespace_id="instance")
        try:
            default = await prompts.get(ctx, "formation")
            assert "source_text" in default["variable_names"]
            await prompts.save(ctx, "formation", "Remember {{ source_text }}", "edit-one")
            preview = prompts.preview(
                {"id": "formation", "revision": "draft", "template": "Draft {{ text }}"},
                {"text": "真实材料"},
            )
            assert preview["rendered"] == "Draft 真实材料"
            with pytest.raises(UndefinedError):
                prompts.preview(
                    {"id": "formation", "revision": "draft", "template": "{{ missing }}"},
                    {},
                )
            assert (await prompts.get(ctx, "formation"))["revision"] == 1
            await prompts.save(ctx, "formation", "Changed {{ source_text }}", "edit-two")
            await prompts.activate(ctx, "formation", 1, "restore-one")
            result = await prompts.render(ctx, {"source_text": "原件"}, "formation")
            assert result["rendered"] == "Remember 原件"
            history = await prompts.history(ctx, "formation")
            assert [row["revision"] for row in history] == [2, 1, 0]
            assert [row["revision"] for row in history if row["active"]] == [1]
            await prompts.activate(ctx, "formation", 0, "restore-default")
            assert (await prompts.get(ctx, "formation"))["template_hash"] == default[
                "template_hash"
            ]
        finally:
            await authority.close()
        reopened = Authority(path)
        await reopened.initialize()
        try:
            value = await PromptService(reopened).get(ctx, "formation")
            assert value["revision"] == 0 and value["source"] == "bundled"
        finally:
            await reopened.close()

    asyncio.run(scenario())


def test_text_and_media_share_one_formation_role_budget(tmp_path: Path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        ledger = CallLedger(authority)
        await authority.initialize()
        ctx = CallContext(caller_id="worker", namespace_id="research", subject_id="owner")
        try:
            call_id, _ = await ledger.reserve(
                ctx, "media.formation", {"input": "image"}, 1, budget_role="formation"
            )
            await ledger.finish(ctx, call_id, {"result": "retained"}, {})
            reused, output = await ledger.reserve(
                ctx, "media.formation", {"input": "image"}, 1, budget_role="formation"
            )
            assert reused == call_id and output == {"result": "retained"}
            with pytest.raises(NousError) as caught:
                await ledger.reserve(ctx, "formation", {"input": "text"}, 1)
            assert caught.value.code == "ResourceExhausted"
            # Description has its own configured role allowance.
            await ledger.reserve(
                ctx, "media.description", {"input": "image"}, 1, budget_role="description"
            )
        finally:
            await authority.close()

    asyncio.run(scenario())


def test_http_backup_restores_a_new_instance_only(tmp_path: Path):
    async def scenario():
        import httpx

        from nous.platform.configuration import Settings
        from nous.transports.http import create_app

        settings = Settings(instance=tmp_path / "original")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app),
                base_url="http://test",
                headers={"Authorization": "Bearer " + settings.token_file.read_text().strip()},
            ) as client:
                created = await client.post("/api/backups")
                assert created.status_code == 200
                archive = await client.get("/api/backups/" + created.json()["filename"])
                assert archive.status_code == 200
                restored = await client.post(
                    "/api/backups/restore",
                    params={"instance_name": "restored"},
                    content=archive.content,
                    headers={"Content-Type": "application/zip"},
                )
                assert restored.status_code == 200
                value = restored.json()
                target = tmp_path / "restored"
                assert value["instance"] == str(target)
                assert (target / "data" / "authority.sqlite").is_file()
                new_token = await asyncio.to_thread(Path(value["credential_file"]).read_text)
                original_token = await asyncio.to_thread(settings.token_file.read_text)
                assert new_token != original_token
                again = await client.post(
                    "/api/backups/restore",
                    params={"instance_name": "restored"},
                    content=archive.content,
                )
                assert again.status_code == 409
                escaped = await client.post(
                    "/api/backups/restore",
                    params={"instance_name": "../escape"},
                    content=archive.content,
                )
                assert escaped.status_code == 400
                invalid = await client.post(
                    "/api/backups/restore",
                    params={"instance_name": "invalid"},
                    content=b"broken archive",
                )
                assert invalid.status_code == 400 and not (tmp_path / "invalid").exists()

    asyncio.run(scenario())
