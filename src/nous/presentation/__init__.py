"""Instance-local gettext presentation; machine values keep their original meaning."""

import gettext
import json
from collections.abc import Mapping
from functools import lru_cache
from importlib.resources import files
from typing import cast

from nous.runtime.errors import NousError
from nous.temporal import Instant, Interval, Unknown
from nous.temporal import format_time as _format_time

_LABELS = {
    "state": "State",
    "instance_id": "Instance ID",
    "name": "Name",
    "instance": "Instance",
    "display_name": "Name",
    "layout": "Layout",
    "config_file": "Configuration file",
    "file": "File",
    "initialized": "Initialized",
    "in_use": "In use",
    "diagnostic": "Diagnostic",
    "paths": "Directories",
    "config": "Configuration",
    "data": "Data",
    "content": "Content",
    "index": "Index",
    "cache": "Cache",
    "logs": "Logs",
    "temp": "Temporary files",
    "runtime": "Runtime",
    "exports": "Exports",
    "capabilities": "Capabilities",
    "configuration": "Configuration",
    "database": "Database",
    "execution": "Execution",
    "time": "Time",
    "models": "Models",
    "network": "Network",
    "durable_execution": "Durable execution",
    "config_version": "Configuration version",
    "in_flight": "Operations in flight",
    "bindings": "Persistent bindings",
    "observability": "Observability",
    "trace_export": "Trace export",
    "metrics_export": "Metrics export",
    "version": "Version",
    "pending_restart": "Restart required",
    "changes": "Changes",
    "path": "Field",
    "source": "Source",
    "layer": "Layer",
    "location": "Location",
    "shadowed": "Overridden",
    "saved_value": "Saved value",
    "desired_value": "Desired value",
    "effective_value": "Effective value",
    "definition": "Definition",
    "values": "Values",
    "valid": "Valid",
    "checks": "Checks",
    "shape": "Structure",
    "references": "References",
    "model_execution": "Model execution",
    "journal_mode": "Journal mode",
    "foreign_keys": "Foreign keys",
    "schema_version": "Schema version",
    "input": "Input",
    "domain": "Time domain",
    "timezone": "Time zone",
    "value": "Value",
    "details": "Details",
    "database_id": "Database ID",
    "effective_version": "Effective version",
    "saved_version": "Saved version",
    "file_saved": "File saved",
    "write_performed": "File write performed",
    "activation_failure": "Activation failure",
    "phase": "Phase",
    "binding_application": "Binding application",
    "live_application": "Live application",
    "cause_code": "Cause code",
    "cause_details": "Cause details",
    "recovery": "Recovery",
    "operation": "Operation",
    "message": "Message",
    "fields": "Fields",
}
_ERROR_LABELS = {
    "instance": "Instance operation failed",
    "configuration": "Configuration operation failed",
    "storage": "Storage operation failed",
    "time": "Time operation failed",
    "operation": "Operation failed",
    "input": "Invalid input",
}


def select_language(explicit: str | None = None, configured: str | None = None) -> str:
    selected = explicit or configured or "zh-CN"
    if selected not in {"zh-CN", "en"}:
        raise NousError("configuration.invalid_language", "Language must be zh-CN or en")
    return selected


@lru_cache(maxsize=2)
def _translation(language: str) -> gettext.NullTranslations:
    if language == "en":
        return gettext.NullTranslations()
    resource = files(__package__).joinpath("locales", "zh_CN", "LC_MESSAGES", "nous.mo")
    with resource.open("rb") as stream:
        translated = gettext.GNUTranslations(stream)
    translated.add_fallback(gettext.NullTranslations())
    return translated


def translate(message: str, language: str = "zh-CN") -> str:
    selected = select_language(language)
    return _translation(selected).gettext(message) if message else ""


def error_message(error: NousError, language: str = "zh-CN") -> str:
    """Translate a supplied message while retaining unknown diagnostic information."""
    text = translate(error.message, language)
    if language == "en" or text != error.message:
        return text
    label = _ERROR_LABELS.get(error.code.partition(".")[0])
    return f"{translate(label, language)}: {text}" if label else text


def render_error(error: NousError, language: str = "zh-CN") -> str:
    lines = [f"[{error.code}] {error_message(error, language)}"]
    if error.details:
        if error.code in {
            "configuration.apply_failed",
            "configuration.save_failed",
            "configuration.read_failed",
        }:
            lines.append(format_status(error.details, language))
        else:
            lines.append(
                f"{translate('Details', language)}: {json.dumps(error.details, ensure_ascii=False)}"
            )
    return "\n".join(lines)


def format_status(result: Mapping[str, object], language: str = "zh-CN") -> str:
    """Render known management labels, preserving paths, identifiers and user text."""
    language = select_language(language)
    lines: list[str] = []

    def render(values: Mapping[str, object], indent: int = 0, *, statuses: bool = False) -> None:
        for key, value in values.items():
            label = translate(_LABELS.get(key, key), language)
            prefix = " " * indent + label + ":"
            if isinstance(value, Mapping) and key not in {
                "values",
                "details",
                "definition",
                "cause_details",
            }:
                lines.append(prefix)
                render(
                    cast(Mapping[str, object], value),
                    indent + 2,
                    statuses=key in {"capabilities", "checks"},
                )
            else:
                if isinstance(value, bool):
                    text = translate("Yes" if value else "No", language)
                elif isinstance(value, str):
                    text = (
                        translate(value, language).replace("_", " ")
                        if (
                            statuses
                            or key
                            in {
                                "state",
                                "bindings",
                                "model_execution",
                                "phase",
                                "binding_application",
                                "live_application",
                                "recovery",
                            }
                        )
                        else value
                    )
                else:
                    text = json.dumps(value, ensure_ascii=False, default=str)
                lines.append(prefix + " " + text)

    render(result)
    return "\n".join(lines)


def format_time(
    value: Unknown | Instant | Interval, language: str = "zh-CN", timezone: str = "UTC"
) -> str:
    return _format_time(value, select_language(language), timezone)


__all__ = [
    "error_message",
    "format_status",
    "format_time",
    "render_error",
    "select_language",
    "translate",
]
