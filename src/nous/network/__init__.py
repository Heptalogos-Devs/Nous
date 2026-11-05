"""Runtime-owned HTTP and protocol clients for configured model providers."""

import ssl
from collections.abc import Mapping
from pathlib import Path

import httpx2
from openai import AsyncOpenAI

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot, NousConfig
from nous.configuration.definitions import NetworkConfig
from nous.credentials import Credentials


def _timeout(settings: NetworkConfig) -> httpx2.Timeout:
    return httpx2.Timeout(
        connect=settings.connect_timeout_seconds,
        read=settings.read_timeout_seconds,
        write=settings.write_timeout_seconds,
        pool=settings.pool_timeout_seconds,
    )


async def _omit_authorization(request: httpx2.Request) -> None:
    request.headers.pop("Authorization", None)


class ModelConnections:
    """Lazily create one SDK/HTTP connection per configured provider and own its closure."""

    def __init__(
        self,
        instance_directory: Path,
        settings: NousConfig,
        environment: Mapping[str, str] | None = None,
    ) -> None:
        self._directory = instance_directory
        self._settings = settings
        self._credentials = Credentials(instance_directory, settings.credentials, environment)
        self._clients: dict[str, AsyncOpenAI] = {}

    def client(self, provider_id: str) -> AsyncOpenAI:
        """Borrow the runtime-owned client for a configured provider.

        Connection and credential settings are captured at startup; the owner
        closes the shared client. SDK retries are disabled so request attempts
        are governed by the model executor. Unknown providers and unsupported
        protocols raise ``NousError``.
        """
        existing = self._clients.get(provider_id)
        if existing is not None:
            return existing
        provider = self._settings.providers.get(provider_id)
        if provider is None:
            raise NousError(
                "model.provider_unknown",
                "The model provider is not configured.",
                details={"provider": provider_id},
            )
        if provider.protocol != "openai-chat-completions":
            raise NousError(
                "model.protocol_unsupported",
                "The configured model protocol is not supported.",
                details={"provider": provider_id, "protocol": provider.protocol},
            )
        network = self._settings.provider_network(provider_id)
        verify: ssl.SSLContext | bool = True
        if network.ca_file:
            ca_file = Path(network.ca_file).expanduser()
            if not ca_file.is_absolute():
                ca_file = self._directory / ca_file
            verify = ssl.create_default_context(cafile=str(ca_file))
        key = (
            self._credentials.resolve(provider.credential_ref).get_secret_value()
            if provider.credential_ref
            else ""
        )
        transport = httpx2.AsyncClient(
            proxy=network.proxy_url,
            verify=verify,
            trust_env=network.trust_env,
            timeout=_timeout(network),
            limits=httpx2.Limits(max_connections=network.max_connections),
            event_hooks={"request": [_omit_authorization]} if not key else None,
        )
        try:
            client = AsyncOpenAI(
                base_url=provider.endpoint,
                api_key=key or "anonymous",
                admin_api_key="",
                organization="",
                project="",
                webhook_secret="",
                default_headers={"Authorization": f"Bearer {key or 'anonymous'}"},
                http_client=transport,
                timeout=_timeout(network),
                # The model executor counts every local submission across all retry causes.
                max_retries=0,
            )
        except BaseException:
            # Construction has not handed the transport to the runtime yet.
            # No request has started, so there is no open socket to close here.
            raise
        self._clients[provider_id] = client
        return client

    def timeout(self, provider_id: str, snapshot: ConfigSnapshot) -> httpx2.Timeout:
        """Read per-request timeouts from the operation snapshot while retaining its shared pool."""
        settings = snapshot.typed_values()
        provider = settings.providers.get(provider_id)
        if provider is None:
            raise NousError(
                "model.provider_unknown",
                "The model provider is not configured.",
                details={"provider": provider_id},
            )
        return _timeout(settings.provider_network(provider_id))

    async def aclose(self) -> None:
        """Attempt every client closure before propagating the first shutdown failure."""
        clients, self._clients = self._clients, {}
        failure: BaseException | None = None
        for client in clients.values():
            try:
                await client.close()
            except BaseException as error:
                if failure is None:
                    failure = error
                else:
                    failure.add_note(f"Another connection also failed to close: {error!r}")
        if failure is not None:
            raise failure
