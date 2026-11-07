"""Side-effect-free descriptions of the two durable cognitive workflows."""

from nous.jobs.configuration import JobsConfig
from nous.jobs.contracts import QueueDefinition

CLASS_NAME = "NousMaintenance"
ANNOTATION = "nous.annotation"
REFRESH = "nous.refresh"


def queues(settings: JobsConfig) -> tuple[QueueDefinition, ...]:
    return (
        QueueDefinition(
            "annotation",
            "nous-annotation",
            ANNOTATION,
            CLASS_NAME,
            settings.annotation_concurrency,
        ),
        QueueDefinition(
            "refresh",
            "nous-refresh",
            REFRESH,
            CLASS_NAME,
            settings.refresh_concurrency,
        ),
    )
