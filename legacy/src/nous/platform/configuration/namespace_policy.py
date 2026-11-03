"""Configuration declarations owned by admission and formation."""

from nous_sdk.contracts.namespace_policy import FormationPolicy, NamespaceQuota

from .service import ConfigurationEntry


def entries() -> dict[str, ConfigurationEntry]:
    return {
        "namespace.quota": ConfigurationEntry(
            NamespaceQuota(), instance_management=True, scopes=("namespace",)
        ),
        "formation": ConfigurationEntry(FormationPolicy()),
    }
