"""Fluent messages and Babel formatting at user-facing presentation boundaries."""

from __future__ import annotations

from functools import lru_cache
from importlib.resources import files

from babel import Locale, default_locale
from babel.core import UnknownLocaleError
from babel.numbers import format_decimal
from fluent.runtime import FluentBundle, FluentResource


class Localizer:
    def __init__(self, preferred: str | None = None):
        selected = (preferred or default_locale() or "en-US").split(",", 1)[0].split(";", 1)[0]
        try:
            language = Locale.parse(selected.replace("-", "_")).language
        except ValueError, LookupError, UnknownLocaleError:
            language = "en"
        self.locale = "zh-CN" if language == "zh" else "en-US"
        self._bundle = _bundle(self.locale)

    def has(self, key: str) -> bool:
        return self._bundle.has_message(key)

    def text(self, key: str, **values: str | int | float) -> str:
        message = self._bundle.get_message(key)
        if message.value is None:
            raise ValueError(f"Message {key!r} has no display value")
        value, errors = self._bundle.format_pattern(message.value, values)
        if errors:
            raise ValueError(f"Invalid localized message {key!r}") from errors[0]
        return str(value)

    def number(self, value: int | float) -> str:
        return format_decimal(value, locale=self.locale.replace("-", "_"))


@lru_cache(maxsize=2)
def _bundle(locale: str) -> FluentBundle:
    bundle = FluentBundle([locale], use_isolating=False)
    resource = (
        files("nous").joinpath("resources", "locales", f"{locale}.ftl").read_text(encoding="utf-8")
    )
    bundle.add_resource(FluentResource(resource))
    return bundle
