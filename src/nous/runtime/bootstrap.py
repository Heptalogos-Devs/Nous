"""Construct and close exactly the resources used by a local instance."""

from collections.abc import AsyncGenerator, Mapping
from contextlib import AsyncExitStack, ExitStack, asynccontextmanager
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import cast
from uuid import uuid4

import tomlkit

from nous.application.config_operations import OfflineCalls
from nous.application.configuration import ConfigurationOperations
from nous.application.contracts import BoundOperation
from nous.application.dispatch import Application
from nous.application.embedding import PROBE_EMBEDDING, RUN_EMBEDDING, EmbeddingOperations
from nous.cognition.annotation.operations import (
    ADOPT_ANNOTATION,
    ANNOTATE_RECORD,
    PROPOSE_ANNOTATION,
    READ_ANNOTATION,
    READ_ANNOTATION_PROPOSAL,
    AnnotationOperations,
)
from nous.cognition.annotation.task import ConceptAnnotationTask
from nous.cognition.concepts import (
    CREATE_CONCEPT,
    FIND_CONCEPTS,
    READ_CONCEPT,
    REVISE_CONCEPT,
    ConceptOperations,
)
from nous.cognition.feedback import (
    LIST_FEEDBACK,
    READ_FEEDBACK,
    RECORD_FEEDBACK,
    RECORD_QUERY_FEEDBACK,
    RESOLVE_FEEDBACK,
    FeedbackOperations,
)
from nous.cognition.maintenance.operations import MAINTAIN_RECORD, MaintenanceOperations
from nous.cognition.retrieval.operations import QUERY, REFRESH, CognitiveRetrieval
from nous.cognition.scope import (
    CREATE_NAMESPACE,
    LIST_NAMESPACES,
    RENAME_NAMESPACE,
    SELECT_NAMESPACE,
    NamespaceOperations,
)
from nous.cognition.scope import initialize_in as initialize_namespaces_in
from nous.cognition.sources import SourceResolver
from nous.cognition.writing.drafts import DraftStore
from nous.cognition.writing.generation import (
    DRAFT_FROM_RESULT,
    READ_DRAFT,
    REVISE_DRAFT,
    WRITE_DRAFT,
    WritingOperations,
)
from nous.cognition.writing.operations import (
    ADD_NOTE,
    ADOPT_DRAFT,
    APPLY_EDITS,
    ARCHIVE_RECORD,
    CREATE_JOURNAL,
    DIFF_RECORDS,
    HISTORY_RECORD,
    LIST_RECORDS,
    READ_RECORD,
    REPLACE_RECORD,
    RESTORE_RECORD,
    SET_RECALL,
    RecordOperations,
)
from nous.cognition.writing.reading import RecordReader
from nous.cognition.writing.records import WrittenRecords
from nous.cognition.writing.tasks import writing_tasks
from nous.common.errors import NousError
from nous.configuration import Configuration
from nous.content.artifacts import ArtifactStore
from nous.content.operations import (
    ACCEPT_CONTENT,
    EXPORT_CONTENT,
    READ_ARTIFACT,
    READ_RESULT,
    READ_SELECTIONS,
    ContentOperations,
)
from nous.content.results import ResultStore
from nous.media.analysis import ANALYZE_MEDIA, INSPECT_MEDIA, MediaAnalysis
from nous.media.preparation import MediaPreparer
from nous.models.embedding_store import EmbeddingSpaceRegistry
from nous.models.embeddings import EmbeddingExecutor
from nous.models.executor import ModelExecutor
from nous.models.generation import GenerationOperations
from nous.models.generation.operations import MODEL_DISCOVER, MODEL_PROBE, MODEL_RUN, PROMPT_PREVIEW
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
from nous.wave.operations import (
    WAVE_BUILD,
    WAVE_CLEANUP,
    WAVE_INSPECT,
    WAVE_QUERY,
    WAVE_READ,
    WAVE_SEGMENT,
    WaveOperations,
)

from .bindings import InstanceBinding, prepare_resource_directories
from .execution import ExecutionManager
from .instance import InstanceDescriptor, InstanceLocator, InstanceLock
from .operations import (
    CANCEL_OPERATION,
    CHECK_RUNTIME,
    LIST_OPERATIONS,
    READ_OPERATION,
    WAIT_OPERATION,
    RuntimeOperations,
)
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
    use ``app`` to invoke complete operations.
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
    records: RecordOperations
    writing: WritingOperations
    app: Application = field(init=False)
    state: str = "starting"

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
    allow_background: bool = False,
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
            allow_background=allow_background,
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
                drafts = DraftStore(database, execution, written_records, record_reader, results)
                feedback = FeedbackOperations(
                    database, execution, namespaces, record_reader, wave, clock
                )
                maintenance = MaintenanceOperations(
                    database,
                    execution,
                    namespaces,
                    record_reader,
                    annotations,
                    cognitive_retrieval,
                )
                records = RecordOperations(
                    written_records, record_reader, drafts, feedback, maintenance
                )
                writing = WritingOperations(
                    written_records, record_reader, generation, drafts, records
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
                    records=records,
                    writing=writing,
                    state="ready",
                )
                content_operations = ContentOperations(results, artifacts)
                runtime_operations = RuntimeOperations(execution, runtime.check)
                configuration_calls = OfflineCalls(
                    configuration,
                    prompts,
                    execution,
                    temp=paths.temp,
                    writer=configuration_operations,
                )
                operations = (
                    BoundOperation(ACCEPT_CONTENT, content_operations.accept),
                    BoundOperation(READ_ARTIFACT, content_operations.read_artifact),
                    BoundOperation(READ_RESULT, content_operations.read_result),
                    BoundOperation(READ_SELECTIONS, content_operations.read_selections),
                    BoundOperation(EXPORT_CONTENT, content_operations.export_offer),
                    BoundOperation(CHECK_RUNTIME, runtime_operations.check),
                    BoundOperation(LIST_OPERATIONS, runtime_operations.list),
                    BoundOperation(READ_OPERATION, runtime_operations.read),
                    BoundOperation(WAIT_OPERATION, runtime_operations.wait),
                    BoundOperation(CANCEL_OPERATION, runtime_operations.cancel),
                    BoundOperation(CREATE_CONCEPT, concepts.create),
                    BoundOperation(READ_CONCEPT, concepts.read),
                    BoundOperation(FIND_CONCEPTS, concepts.find),
                    BoundOperation(REVISE_CONCEPT, concepts.revise),
                    BoundOperation(PROPOSE_ANNOTATION, annotations.propose),
                    BoundOperation(ADOPT_ANNOTATION, annotations.adopt),
                    BoundOperation(ANNOTATE_RECORD, annotations.annotate),
                    BoundOperation(READ_ANNOTATION, annotations.read_annotation),
                    BoundOperation(READ_ANNOTATION_PROPOSAL, annotations.read_annotation_proposal),
                    BoundOperation(MAINTAIN_RECORD, maintenance.run),
                    BoundOperation(QUERY, cognitive_retrieval.handle_query),
                    BoundOperation(REFRESH, cognitive_retrieval.handle_refresh),
                    BoundOperation(RECORD_QUERY_FEEDBACK, feedback.record_query),
                    BoundOperation(RECORD_FEEDBACK, feedback.record),
                    BoundOperation(READ_FEEDBACK, feedback.read),
                    BoundOperation(LIST_FEEDBACK, feedback.list),
                    BoundOperation(RESOLVE_FEEDBACK, feedback.resolve),
                    BoundOperation(MODEL_RUN, generation.run),
                    BoundOperation(MODEL_DISCOVER, generation.discover),
                    BoundOperation(PROBE_EMBEDDING, embedding.probe),
                    BoundOperation(RUN_EMBEDDING, embedding.run),
                    BoundOperation(MODEL_PROBE, generation.probe),
                    BoundOperation(PROMPT_PREVIEW, generation.preview),
                    BoundOperation(ANALYZE_MEDIA, media.analyze),
                    BoundOperation(INSPECT_MEDIA, media.inspect),
                    BoundOperation(WAVE_BUILD, wave.handle_build),
                    BoundOperation(WAVE_QUERY, wave.handle_query),
                    BoundOperation(WAVE_READ, wave.handle_read),
                    BoundOperation(WAVE_INSPECT, wave.handle_inspect),
                    BoundOperation(WAVE_SEGMENT, wave.handle_segment),
                    BoundOperation(WAVE_CLEANUP, wave.handle_cleanup),
                    BoundOperation(CREATE_NAMESPACE, namespaces.create),
                    BoundOperation(LIST_NAMESPACES, namespaces.list),
                    BoundOperation(SELECT_NAMESPACE, namespaces.select),
                    BoundOperation(RENAME_NAMESPACE, namespaces.rename),
                    BoundOperation(CREATE_JOURNAL, records.create_journal),
                    BoundOperation(ADD_NOTE, records.add_note),
                    BoundOperation(READ_RECORD, records.read_record),
                    BoundOperation(REPLACE_RECORD, records.replace_record),
                    BoundOperation(APPLY_EDITS, records.apply_edits),
                    BoundOperation(ARCHIVE_RECORD, records.archive_record),
                    BoundOperation(RESTORE_RECORD, records.restore_record),
                    BoundOperation(SET_RECALL, records.set_recall),
                    BoundOperation(HISTORY_RECORD, records.history_record),
                    BoundOperation(LIST_RECORDS, records.list_records),
                    BoundOperation(DIFF_RECORDS, records.diff_records),
                    BoundOperation(ADOPT_DRAFT, records.adopt_draft),
                    BoundOperation(WRITE_DRAFT, writing.generate),
                    BoundOperation(DRAFT_FROM_RESULT, writing.draft_from_result),
                    BoundOperation(READ_DRAFT, writing.read_draft),
                    BoundOperation(REVISE_DRAFT, writing.revise_draft),
                )
                runtime.app = Application(
                    cast(tuple[BoundOperation[object, object], ...], operations)
                    + configuration_calls.bindings(writable=True),
                    execution=execution,
                    artifacts=artifacts,
                    results=results,
                    readers=content.file_readers,
                    chunk_size=settings.content.io_chunk_bytes,
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
