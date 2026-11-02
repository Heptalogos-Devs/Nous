"""Measurement of the complete actual model request by the model owner."""

from pydantic import Field, JsonValue

from .common import Contract


class ModelMeasurement(Contract):
    input_tokens: int = Field(ge=0)
    output_reserve: int = Field(ge=0)
    context_window: int | None = Field(default=None, gt=0)
    method: str
    estimated: bool
    media_items: tuple[dict[str, JsonValue], ...] = ()

    @property
    def complete(self) -> bool:
        return all(item.get("tokens") is not None for item in self.media_items)

    def host_measurement(self):
        from .context import HostInputMeasurement

        return HostInputMeasurement(
            input_tokens=self.input_tokens,
            reserved_output_tokens=self.output_reserve,
            context_window_tokens=self.context_window,
            source=self.method + (":estimated" if self.estimated else ":measured"),
        )
