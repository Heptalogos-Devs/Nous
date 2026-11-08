"""Construct and close exactly the resources used by a local instance."""

from collections.abc import AsyncGenerator, Mapping
from contextlib import AsyncExitStack, ExitStack, asynccontextmanager
from dataclasses import asdict, dataclass, field
from pathlib import Path
from uuid import uuid4

import tomlkit

from nous.application.configuration import ConfigurationOperations
from nous.application.embedding import EmbeddingOperations
from nous.cognition.annotation.operations import AnnotationOperations
from nous.cognition.annotation.task import ConceptAnnotationTask
from nous.cognition.concepts import ConceptOperations
from nous.cognition.contracts import NamespaceRef
from nous.cognition.feedback import FeedbackOperations
from nous.cognition.maintenance import MaintenanceOperations
from nous.cognition.maintenance.definitions import queues as maintenance_queues
from nous.cognition.retrieval.operations import CognitiveRetrieval
from nous.cognition.scope import NamespaceOperations
from nous.cognition.scope import initialize_in as initialize_namespaces_in
from nous.cognition.services import CognitiveServices
from nous.cognition.sources import SourceResolver
from nous.cognition.writing.drafts import DraftStore
from nous.cognition.writing.generation import WritingOperations
from nous.cognition.writing.operations import RecordOperations
from nous.cognition.writing.reading import RecordReader
from nous.cognition.writing.records import WrittenRecords
from nous.cognition.writing.tasks import writing_tasks
from nous.common.errors import NousError
from nous.configuration import Configuration
from nous.content.artifacts import ArtifactStore
from nous.content.results import ResultStore
from nous.jobs.operations import JobOperations
from nous.jobs.runtime import JobRuntime, initialize_jobs, open_jobs
from nous.media.analysis import MediaAnalysis
from nous.media.preparation import MediaPreparer
from nous.models.embedding_store import EmbeddingSpaceRegistry
from nous.models.embeddings import EmbeddingExecutor
from nous.models.executor import ModelExecutor
from nous.models.generation import GenerationOperations
from nous.models.prompts import PromptCatalog, PromptCompiler
from nous.models.tasks import TaskCatalog, builtin_tasks
from nous.network import ModelConnections
from nous.observability.assembly import LoggingSession, Observability
from nous.observability.diagnostics import recent_content_preparations, recent_problems
from nous.observability.instrumentation import get_logger, operation
from nous.storage.content import ContentStore
from nous.storage.database import Database
from nous.temporal.domains import TimeService, bind_domains, check_domains
from nous.temporal.values import Clock
from nous.wave.operations import WaveOperations

from .bindings import InstanceBinding, prepare_resource_directories
from .execution import ExecutionManager
from .instance import InstanceDescriptor, InstanceLocator, InstanceLock
from .paths import ResolvedPaths
from .schema import BUSINESS_METADATA


def task_catalog() -> TaskCatalog:
    """Compose model tasks with the writing domain's actual task contracts."""
    return TaskCatalog(
        (*builtin_tasks().definitions.values(), *writing_tasks(), ConceptAnnotationTask())
    )


@asynccontextmanager
async def _resources() -> AsyncGenerator[AsyncExitStack]:
    """Keep the initiating failure when resource cleanup also reports a failure."""
    stack = AsyncExitStack()
    try:
        yield stack
    except BaseException as primary:
        try:
            await stack.aclose()
        except BaseException as cleanup:
            primary.add_note(f"Resource cleanup also failed: {cleanup!r}")
        raise
    else:
        await stack.aclose()


@dataclass
class Runtime:
    """Borrow the fully assembled services of one open instance.

    Acquire this object with ``open_instance`` and use its services only inside
    that context. The runtime owns their shared resources and shutdown; callers
    should not close the individual services. Use ``cognition`` to bind work to
    a stable namespace identity.
    """

    paths: ResolvedPaths
    descriptor: InstanceDescriptor
    configuration: ConfigurationOperations
    database: Database
    execution: ExecutionManager
    time: TimeService
    observability: Observability
    artifacts: ArtifactStore
    results: ResultStore
    connections: ModelConnections
    prompts: PromptCatalog
    models: ModelExecutor
    generation: GenerationOperations
    media: MediaAnalysis
    embeddings: EmbeddingExecutor
    embedding_spaces: EmbeddingSpaceRegistry
    embedding: EmbeddingOperations
    wave: WaveOperations
    namespaces: NamespaceOperations
    written_records: WrittenRecords
    record_reader: RecordReader
    drafts: DraftStore
    concepts: ConceptOperations
    annotations: AnnotationOperations
    cognitive_retrieval: CognitiveRetrieval
    feedback: FeedbackOperations
    maintenance: MaintenanceOperations
    jobs: JobOperations
    job_runtime: JobRuntime
    state: str = "starting"
    _cognition: dict[str, CognitiveServices] = field(default_factory=dict)

    async def cognition(self, namespace: str | NamespaceRef | None = None) -> CognitiveServices:
        """Bind shared services to a resolved immutable namespace identity.

        Args:
            namespace: Namespace identity or selector; None or an empty selector
                uses the current configured default. The namespace must exist.

        Returns:
            A runtime-owned service set cached by resolved namespace identity.
        """
        selected = namespace or self.configuration.snapshot.values.cognition.default_namespace
        async with self.database.read() as tx:
            identity = (await self.namespaces.get_in(tx, selected)).ref
        cached = self._cognition.get(identity.id)
        if cached is not None:
            return cached
        reader = RecordReader(
            self.database, self.execution, self.artifacts, self.namespaces, identity
        )
        concepts = ConceptOperations(
            self.database,
            self.execution,
            self.namespaces,
            self.descriptor.instance_id,
            self.time.clock,
            namespace=identity,
        )
        annotations = AnnotationOperations(
            self.database, self.execution, reader, concepts, self.generation
        )
        retrieval = self.cognitive_retrieval.bind(identity)
        feedback = FeedbackOperations(
            self.database,
            self.execution,
            self.namespaces,
            reader,
            self.wave,
            self.time.clock,
            namespace_id=identity.id,
        )
        drafts = DraftStore(
            self.database,
            self.execution,
            self.written_records,
            reader,
            self.generation,
            self.results,
        )
        records = RecordOperations(
            identity,
            self.written_records,
            reader,
            drafts,
            annotations,
            retrieval,
            feedback,
            self.maintenance,
        )
        writing = WritingOperations(
            identity, self.written_records, reader, self.generation, drafts, records
        )
        services = CognitiveServices(
            identity, records, writing, annotations, concepts, retrieval, feedback
        )
        self._cognition[identity.id] = services
        return services

    def status(self) -> dict[str, object]:
        effective = self.configuration.snapshot.values
        configured_models = bool(effective.models)
        return {
            "state": self.state,
            "instance_id": self.descriptor.instance_id,
            "name": effective.instance.name,
            "config_version": self.configuration.snapshot.version,
            "configuration": self.configuration.status(),
            "capabilities": {
                "configuration": "ready",
                "database": "ready",
                "execution": "ready",
                "time": "ready",
                "models": "configured" if configured_models else "unconfigured",
                "network": "ready",
                "content": "ready",
                "durable_execution": "ready" if self.job_runtime.worker_running else "available",
            },
            "in_flight": self.execution.in_flight,
            "model_execution": "configured" if configured_models else "unconfigured",
            "paths": self.paths.to_dict(),
        }

    async def check(self) -> dict[str, object]:
        """Read actual owners and adopted-result summaries without making model requests."""
        async with self.execution.operation("runtime.check"):
            settings = self.configuration.snapshot.values
            result = self.status()
            result["database"] = asdict(await self.database.check())
            result["models"] = {
                "resources": list(settings.models),
                "configured": bool(settings.models),
                **self.generation.capabilities(self.configuration.snapshot),
            }
            result["media"] = await self.execution.blocking(
                self.media.preparer.capabilities, settings.media
            )
            result["embedding"] = await self.embedding_spaces.status()
            wave_status = await self.wave.status()
            wave_status["recent_preparations"] = (
                await self.execution.blocking(recent_content_preparations, self.paths.logs)
                if settings.logging.file_enabled
                else []
            )
            result["wave"] = wave_status
            result["cognition"] = {
                **await self.record_reader.status(),
                "drafts": await self.drafts.status(),
                "concepts": await self.concepts.status(),
                "annotations": await self.annotations.status(),
                "retrieval": await self.cognitive_retrieval.status(),
            }
            result["execution"] = self.execution.status()
            jobs, _ = await self.jobs.list(limit=20)
            failed_jobs = [job for job in jobs if job.status == "ERROR"][:3]
            failures = [await self.jobs.read(job.ref) for job in failed_jobs]
            result["jobs"] = {
                "worker_running": self.job_runtime.worker_running,
                "active_stages": self.execution.status()["durable_tasks"],
                "queues": await self.jobs.queues(),
                "recent": [job.model_dump(mode="json") for job in jobs],
                "recent_failures": [job.model_dump(mode="json") for job in failures],
            }
            result["recent_results"] = await self.results.recent()
            result["observability"] = {
                "logs": str(self.paths.logs / "nous.jsonl")
                if settings.logging.file_enabled
                else None,
                "recent_problems": await self.execution.blocking(recent_problems, self.paths.logs)
                if settings.logging.file_enabled
                else [],
                "trace_enabled": settings.tracing.enabled,
                "trace_export": settings.tracing.export,
                "local_spans_persisted": False,
                "export_configured": settings.tracing.enabled and settings.tracing.export != "none",
                "trace_sample_ratio": settings.tracing.sample_ratio,
                "metrics_export": settings.metrics.export,
                "metrics_enabled": settings.metrics.enabled,
            }
            result["bindings"] = {
                "time": "checked",
                "embedding": "checked" if settings.embedding.spaces else "unconfigured",
            }
            return result


async def initialize_instance(
    locator: InstanceLocator | Path | str | None = None,
    *,
    layout: str = "portable",
    name: str = "default",
    environment: Mapping[str, str] | None = None,
) -> InstanceDescriptor:
    """Initialize a new instance or verify an existing instance's bindings.

    Args:
        locator: Instance location or selector; None uses normal resolution.
        layout: ``portable`` or ``platform`` for a newly created instance.
        name: Human-readable name for a newly created instance.
        environment: Optional environment mapping used during resolution and
            configuration loading.

    Returns:
        The resolved instance identity and layout.

    Raises:
        NousError: The layout is invalid, the new location is nonempty, or the
            existing instance's resources cannot be verified. Initialization may
            leave created files if a later step fails.
    """
    selected = InstanceLocator.resolve(locator, environment)
    if layout not in {"portable", "platform"}:
        raise NousError(
            "configuration.invalid_value", "Instance layout must be portable or platform."
        )
    existing = selected.config_file.is_file()
    created_identity: str | None = None
    if not existing:
        if selected.directory.exists() and any(selected.directory.iterdir()):
            raise NousError(
                "instance.not_empty",
                "The instance directory contains files and has no valid nous.toml.",
                details={"path": str(selected.directory)},
            )
        selected.directory.mkdir(parents=True, exist_ok=True)
        document = tomlkit.document()
        created_identity = str(uuid4())
        document["instance"] = {"id": created_identity, "name": name, "layout": layout}
        try:
            with selected.config_file.open("x", encoding="utf-8", newline="\n") as stream:
                stream.write(tomlkit.dumps(document))
        except FileExistsError as error:
            raise NousError(
                "instance.in_use", "Another operation is initializing this instance.", cause=error
            ) from error
    configuration = Configuration(selected.config_file, environment=environment)
    settings = configuration.snapshot.values
    if not existing and (
        settings.instance.id != created_identity or settings.instance.layout != layout
    ):
        raise NousError(
            "configuration.readonly",
            "Instance identity and layout are determined by initialization.",
        )
    descriptor = InstanceDescriptor(
        settings.instance.id, settings.instance.name, settings.instance.layout
    )
    paths = ResolvedPaths.resolve(selected, descriptor, settings.paths)
    with InstanceLock(selected.directory):
        if existing:
            binding = InstanceBinding.read(selected)
            binding.check_locations(selected, descriptor, paths, settings.database.filename)
        else:
            prepare_resource_directories(paths)
            binding = InstanceBinding.create(
                selected, descriptor, paths, settings.database.filename
            )
        with InstanceLock(paths.data, data=True):
            if existing:
                binding.check_resources(paths)
            async with Database.open(
                paths.resolve_owned("data", settings.database.filename),
                settings.database,
                descriptor.instance_id,
                create=not existing,
                database_id=str(binding.database_id),
                additional_metadata=BUSINESS_METADATA,
            ) as database:
                await bind_domains(database, settings.time.domains)
                if not existing:
                    async with database.write() as tx:
                        default_namespace = await initialize_namespaces_in(
                            tx, descriptor.instance_id
                        )
                    change = configuration.prepare_change(
                        {"cognition": {"default_namespace": default_namespace.id}},
                        (),
                        "instance",
                        None,
                        None,
                        reload=False,
                    )
                    candidate = configuration.save_change(change)
                    configuration.publish_change(change, candidate, pending=False)
                await database.check()
                await initialize_jobs(
                    database,
                    descriptor.instance_id,
                    settings.jobs,
                    maintenance_queues(settings.jobs),
                )
                if not existing:
                    binding.write(selected, paths)
            paths.create_runtime_directories()
    return descriptor


@asynccontextmanager
async def open_instance(
    locator: InstanceLocator | Path | str | None = None,
    *,
    environment: Mapping[str, str] | None = None,
    overrides: Mapping[str, object] | None = None,
) -> AsyncGenerator[Runtime]:
    """Open an initialized instance and own its resources until context exit.

    Args:
        locator: Instance location or selector; None uses normal resolution.
        environment: Optional environment mapping used for configuration.
        overrides: Optional process-local configuration overrides.

    Yields:
        A ready runtime whose services share this instance's resources.

    Startup checks persistent bindings and obtains instance locks. Exit stops and
    waits for operations before closing connections, storage and locks. Opening
    does not initialize a missing instance; call ``initialize_instance`` first.
    """
    from .local_context import current_call

    call = current_call.get()
    if call is not None:
        selected = InstanceLocator.resolve(locator, environment)
        if (
            selected.directory != call.locator.directory
            or environment is not None
            or overrides is not None
        ):
            raise NousError(
                "instance.selection_conflict", "A local host request can only use its own instance"
            )
        yield call.runtime
        return

    startup_id = str(uuid4())
    logs = LoggingSession(startup_id)
    runtime: Runtime | None = None
    async with _resources() as stack:
        leases = ExitStack()
        stack.callback(leases.close)
        stack.callback(logs.close)
        selected = InstanceLocator.resolve(locator, environment)
        get_logger(__name__).info("runtime.starting")
        configuration = Configuration(
            selected.config_file, environment=environment, overrides=overrides, running=True
        )
        settings = configuration.snapshot.values
        descriptor = InstanceDescriptor(
            settings.instance.id, settings.instance.name, settings.instance.layout
        )
        paths = ResolvedPaths.resolve(selected, descriptor, settings.paths)
        leases.enter_context(InstanceLock(selected.directory))
        binding = InstanceBinding.read(selected)
        binding.check_locations(selected, descriptor, paths, settings.database.filename)
        leases.enter_context(InstanceLock(paths.data, data=True))
        binding.check_resources(paths)
        paths.create_runtime_directories()
        logs.configure(settings.logging, paths.logs, descriptor.instance_id)
        observability = Observability(
            settings.tracing, settings.metrics, descriptor.instance_id, logs
        )
        stack.callback(observability.close)
        storage = await stack.enter_async_context(AsyncExitStack())
        execution = ExecutionManager(
            settings.execution,
            lambda: configuration.snapshot,
            observability_scope=lambda: observability.scope(descriptor.instance_id, startup_id),
        )
        connections = ModelConnections(selected.directory, settings, configuration.environment)
        stack.push_async_callback(connections.aclose)
        wave: WaveOperations | None = None
        content: ContentStore | None = None

        async def close_execution() -> None:
            with (
                observability.scope(descriptor.instance_id, startup_id),
                operation("runtime.shutdown"),
            ):
                try:
                    await execution.drain(
                        configuration.snapshot.values.runtime.shutdown_grace_seconds
                    )
                finally:
                    try:
                        if wave is not None:
                            wave.close()
                    finally:
                        if content is not None:
                            await content.aclose()

        stack.push_async_callback(execution.aclose)
        try:
            with (
                observability.scope(descriptor.instance_id, startup_id),
                operation("runtime.startup"),
            ):
                database = await storage.enter_async_context(
                    Database.open(
                        paths.resolve_owned("data", settings.database.filename),
                        settings.database,
                        descriptor.instance_id,
                        database_id=str(binding.database_id),
                        additional_metadata=BUSINESS_METADATA,
                    )
                )
                job_runtime = await stack.enter_async_context(
                    open_jobs(
                        database,
                        descriptor.instance_id,
                        settings.jobs,
                        execution,
                        maintenance_queues(settings.jobs),
                    )
                )
                # Admitted saves may still enqueue until execution has drained.
                stack.push_async_callback(close_execution)
                embedding_spaces = EmbeddingSpaceRegistry(database)
                await check_domains(database, settings.time.domains)
                await embedding_spaces.check_snapshot(configuration.snapshot)
                await bind_domains(database, settings.time.domains)
                configuration_operations = ConfigurationOperations(
                    configuration,
                    selected,
                    binding,
                    paths,
                    database=database,
                    logs=logs,
                    execution=execution,
                )
                content = ContentStore(
                    paths.content,
                    paths.temp,
                    execution,
                    io_chunk_bytes=settings.content.io_chunk_bytes,
                    reader_limit=settings.execution.max_blocking_workers,
                )
                clock = Clock()
                artifacts = ArtifactStore(
                    database,
                    content,
                    descriptor.instance_id,
                    clock=clock,
                    text_read_max_bytes=lambda: (
                        execution.snapshot.values.content.text_read_max_bytes
                    ),
                    text_cache_root=paths.cache,
                    text_cache_settings=lambda: execution.snapshot.values.content.text_cache,
                )
                results = ResultStore(database, artifacts)
                prompts = PromptCatalog(selected.directory, task_catalog())
                compiler = PromptCompiler(
                    prompts,
                    paths.cache,
                    cache_entries=settings.prompt.cache_entries,
                    bytecode_cache=settings.prompt.bytecode_cache,
                )
                models = ModelExecutor(connections, compiler)
                preparer = MediaPreparer(artifacts, results, execution)
                generation = GenerationOperations(models, results, execution, preparer)
                media = MediaAnalysis(artifacts, preparer, generation, results, execution)
                embeddings = EmbeddingExecutor(connections)
                embedding = EmbeddingOperations(
                    database,
                    execution,
                    artifacts,
                    results,
                    embedding_spaces,
                    embeddings,
                )
                time_service = TimeService(database, clock)
                namespaces = NamespaceOperations(
                    database, execution, configuration_operations, descriptor.instance_id
                )
                record_reader = RecordReader(database, execution, artifacts, namespaces)
                concepts = ConceptOperations(
                    database, execution, namespaces, descriptor.instance_id, clock
                )
                annotations = AnnotationOperations(
                    database, execution, record_reader, concepts, generation
                )
                wave = WaveOperations(
                    database,
                    execution,
                    paths,
                    descriptor.instance_id,
                    time_service,
                    artifacts,
                    results,
                    embedding,
                    preparer,
                )
                cognitive_retrieval = CognitiveRetrieval(
                    database,
                    execution,
                    namespaces,
                    record_reader,
                    artifacts,
                    results,
                    wave,
                    annotations,
                )
                written_records = WrittenRecords(
                    database,
                    artifacts,
                    time_service,
                    namespaces,
                    results,
                    record_reader,
                    SourceResolver(record_reader, preparer),
                )
                drafts = DraftStore(
                    database, execution, written_records, record_reader, generation, results
                )
                feedback = FeedbackOperations(
                    database, execution, namespaces, record_reader, wave, clock
                )
                maintenance = MaintenanceOperations(
                    database,
                    execution,
                    namespaces,
                    record_reader,
                    generation,
                    annotations,
                    cognitive_retrieval,
                    job_runtime.operations,
                )
                runtime = Runtime(
                    paths=paths,
                    descriptor=descriptor,
                    configuration=configuration_operations,
                    database=database,
                    execution=execution,
                    time=time_service,
                    observability=observability,
                    artifacts=artifacts,
                    results=results,
                    connections=connections,
                    prompts=prompts,
                    models=models,
                    generation=generation,
                    media=media,
                    embeddings=embeddings,
                    embedding_spaces=embedding_spaces,
                    embedding=embedding,
                    wave=wave,
                    namespaces=namespaces,
                    written_records=written_records,
                    record_reader=record_reader,
                    drafts=drafts,
                    concepts=concepts,
                    annotations=annotations,
                    cognitive_retrieval=cognitive_retrieval,
                    feedback=feedback,
                    maintenance=maintenance,
                    jobs=job_runtime.operations,
                    job_runtime=job_runtime,
                    state="ready",
                )
            with observability.scope(descriptor.instance_id, startup_id):
                get_logger(__name__).info("runtime.ready")
            yield runtime
        except BaseException:
            if runtime is not None:
                runtime.state = "failed"
            with observability.scope(descriptor.instance_id, startup_id):
                get_logger(__name__).info("runtime.failed")
            raise
        finally:
            if runtime is not None and runtime.state != "failed":
                runtime.state = "stopping"
    if runtime is not None:
        runtime.state = "stopped"
