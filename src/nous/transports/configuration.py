"""Explicit network listeners for the instance host."""

from pydantic import Field, model_validator

from nous.configuration.metadata import ConfigModel, setting


class TransportListener(ConfigModel):
    enabled: bool = setting(False, apply="restart", description="Enable this host listener.")
    host: str = setting("127.0.0.1", min_length=1, apply="restart", description="Listener address.")
    port: int = setting(
        0,
        ge=0,
        le=65535,
        apply="restart",
        description="Listener port; zero selects an available port.",
    )
    credential_ref: str | None = setting(
        None, min_length=1, apply="restart", description="Bearer credential reference."
    )
    tls_certfile: str | None = setting(
        None, min_length=1, apply="restart", description="TLS certificate path."
    )
    tls_keyfile: str | None = setting(
        None, min_length=1, apply="restart", description="TLS private key path."
    )

    @model_validator(mode="after")
    def enabled_has_credential(self) -> TransportListener:
        if self.enabled and not self.credential_ref:
            raise ValueError("An enabled listener requires a credential reference")
        if bool(self.tls_certfile) != bool(self.tls_keyfile):
            raise ValueError("TLS certificate and private key must be configured together")
        return self


class TransportConfig(ConfigModel):
    http: TransportListener = Field(default_factory=TransportListener)
    mcp: TransportListener = Field(default_factory=TransportListener)
    grpc: TransportListener = Field(default_factory=TransportListener)
