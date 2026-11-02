"""The bound SDK carries one live Runtime session through the HTTP host callbacks."""

import asyncio

import httpx
from nous_sdk import NousClient
from nous_sdk.contracts.context import ContextUsage
from nous_sdk.contracts.runtime import HostEvent

from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_bound_sdk_runtime_round_trip(tmp_path):
    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            client = NousClient("http://test", credential_file=settings.token_file)
            await client.transport.http.aclose()
            client.transport.http = httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            )
            try:
                created = await client.namespaces.create("runtime-sdk")
                namespace = await client.namespace_by_id(created.id)
                upload_path = tmp_path / "source.txt"
                upload_path.write_text("The user prefers green tea.", encoding="utf-8")
                receipt = await namespace.materials.upload(upload_path, media_type="text/plain")
                observation = await namespace.materials.observe(receipt.result.id)
                assert observation["material_id"] == receipt.result.id

                memory = await namespace.remember("The user prefers green tea.")
                assert (await namespace.memories.get(memory.reference.exact())).reference == (
                    memory.reference
                )

                binding = await namespace.runtime.open(
                    session="sdk-session",
                    usage=ContextUsage(
                        purpose="Answer the current user message.",
                        intent="Use the user's saved preference when relevant.",
                    ),
                    policy="strict",
                    form_long_term=False,
                )
                request = {
                    "model": "local-host-test",
                    "messages": [{"role": "user", "content": "Which tea do I prefer?"}],
                }
                prepared = await binding.before_model(
                    request,
                    events=(
                        HostEvent(
                            event_id="user:question-1",
                            role="user",
                            payload={"content": "Which tea do I prefer?"},
                            text="Which tea do I prefer?",
                        ),
                    ),
                )
                assert prepared.plan is not None
                assert prepared.plan.request["messages"]
                await binding.mark_sent(prepared)
                first = await binding.after_model(
                    prepared,
                    {
                        "id": "response-1",
                        "object": "chat.completion",
                        "created": 1,
                        "model": "local-host-test",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": "Green tea."},
                                "finish_reason": "stop",
                            }
                        ],
                    },
                    text="Green tea.",
                    model="local-host-test",
                    provider="local-host-test",
                    response_id="response-1",
                )
                assert first is not None and first.state == "completed"

                await binding.capture_tool(
                    "tea-lookup-1",
                    {"preference": "green tea"},
                    text="Preference lookup returned green tea.",
                )
                followup = await binding.before_model(
                    {
                        "model": "local-host-test",
                        "messages": [
                            {"role": "user", "content": "Which tea do I prefer?"},
                            {"role": "assistant", "content": "Green tea."},
                        ],
                    }
                )
                assert followup.plan is not None
                await binding.mark_sent(followup)
                second = await binding.after_model(
                    followup,
                    {
                        "id": "response-2",
                        "object": "chat.completion",
                        "created": 2,
                        "model": "local-host-test",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": "You prefer green tea.",
                                },
                                "finish_reason": "stop",
                            }
                        ],
                    },
                    text="You prefer green tea.",
                    model="local-host-test",
                    provider="local-host-test",
                    response_id="response-2",
                )
                assert second is not None and second.state == "completed"
                session = await namespace.runtime.session("sdk-session")
                assert session.host_instances["main"]["info"]["consumer"] == "default"
                await namespace.runtime.close_session("sdk-session")
            finally:
                await client.close()

    asyncio.run(scenario())
