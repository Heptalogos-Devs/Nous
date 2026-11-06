"""Configuration for durable maintenance workers."""

from nous.configuration.metadata import ConfigModel, setting


class JobsConfig(ConfigModel):
    annotation_concurrency: int = setting(
        2, ge=1, apply="next_worker_start", description="Annotation queue concurrency."
    )
    refresh_concurrency: int = setting(
        1, ge=1, apply="next_worker_start", description="Retrieval refresh queue concurrency."
    )
    poll_interval_seconds: float = setting(
        1.0, gt=0, apply="next_worker_start", description="Worker queue polling interval."
    )
    max_executor_threads: int = setting(
        8, ge=1, apply="restart", description="Maximum DBOS executor threads."
    )
    database_pool_size: int = setting(
        4, ge=1, apply="restart", description="Maximum DBOS database connections."
    )
    workflow_timeout_seconds: float | None = setting(
        None, gt=0, apply="next_enqueue", description="Optional workflow execution timeout."
    )
