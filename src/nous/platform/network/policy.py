"""Validated outbound transport policy; provider semantics remain with the SDK."""

from pathlib import Path
from urllib.parse import urlsplit

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator


class NetworkPolicy(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    trust_env: bool = True
    proxy_url: str | None = None
    verify_tls: bool = True
    ca_file: Path | None = None
    connect_timeout_seconds: float = Field(default=10, gt=0, allow_inf_nan=False)
    read_timeout_seconds: float = Field(default=120, gt=0, allow_inf_nan=False)
    write_timeout_seconds: float = Field(default=30, gt=0, allow_inf_nan=False)
    pool_timeout_seconds: float = Field(default=10, gt=0, allow_inf_nan=False)
    max_connections: int = Field(default=100, ge=1)
    max_keepalive_connections: int = Field(default=20, ge=0)
    keepalive_expiry_seconds: float = Field(default=5, gt=0, allow_inf_nan=False)
    follow_redirects: bool = False

    @field_validator("proxy_url")
    @classmethod
    def proxy(cls, value: str | None) -> str | None:
        if value is not None:
            parsed = urlsplit(value)
            if parsed.scheme not in {"http", "https"} or not parsed.hostname or parsed.fragment:
                raise ValueError("Proxy must be an HTTP(S) URL")
            if parsed.username is not None or parsed.password is not None:
                raise ValueError(
                    "Proxy credentials must be supplied through the protected process environment"
                )
        return value

    @model_validator(mode="after")
    def limits(self) -> NetworkPolicy:
        if self.max_keepalive_connections > self.max_connections:
            raise ValueError("Keepalive connections cannot exceed the connection limit")
        if self.ca_file is not None and not self.verify_tls:
            raise ValueError("An explicit CA file requires TLS verification")
        return self
