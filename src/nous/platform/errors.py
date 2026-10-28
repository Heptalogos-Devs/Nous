"""Public failures, mapped once at each transport boundary."""

class NousError(Exception):
    def __init__(self, code: str, message: str, *, status: int = 400) -> None:
        super().__init__(message)
        self.code = code
        self.status = status


class Unavailable(NousError):
    def __init__(self, message: str) -> None:
        super().__init__("Unavailable", message, status=503)
