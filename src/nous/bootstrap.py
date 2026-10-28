"""Own host resources for exactly the active application lifetime."""

from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager

from nous.application import Application
from nous.platform.configuration import Settings
from nous.platform.instance import InstanceLock, initialize


@asynccontextmanager
async def application(settings: Settings) -> AsyncGenerator[Application]:
    initialize(settings.instance)
    with InstanceLock(settings.instance):
        app = Application(settings)
        try:
            await app.start()
            yield app
        finally:
            await app.close()
