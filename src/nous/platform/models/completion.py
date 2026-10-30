"""Only an explicit provider completion can publish generated text as successful."""

from nous_sdk.contracts import DomainError


def require_complete_text(finish_reason: str | None, *, refused: bool = False) -> None:
    if refused or finish_reason != "stop":
        raise DomainError(
            "model_output_incomplete",
            "Provider did not complete the requested text output",
            finish_reason=finish_reason,
            refused=refused or finish_reason == "content_filter",
        )
