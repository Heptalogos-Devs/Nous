"""Explicit resource composition; domain rules stay with their owners."""

from collections.abc import Awaitable, Callable

from nous_sdk.contracts.context import ContextsPolicy
from nous_sdk.contracts.feedback import FeedbackPolicy
from nous_sdk.contracts.maintenance import GrowthPolicy
from nous_sdk.contracts.media_policy import MediaPolicy
from nous_sdk.contracts.vector_projection import VectorProjectionPolicy

from nous.identity.credentials import CredentialsService
from nous.platform.configuration import Settings
from nous.platform.configuration.models import initial_resources
from nous.platform.configuration.service import ConfigurationEntry, ConfigurationService
from nous.platform.execution import DurableExecution, QueueSpec
from nous.platform.models import ModelService
from nous.platform.network.policy import NetworkPolicy
from nous.platform.network.service import NetworkService
from nous.platform.persistence import Authority, BackupService
from nous.platform.prompts import PromptService


class Application:
    def __init__(self, settings: Settings) -> None:
        from nous.cognition.records import CognitiveRecords
        from nous.identity.service import IdentityService
        from nous.material.service import MaterialService
        from nous.retrieval.policy import defaults as retrieval_defaults
        from nous.retrieval.profiles import WavePolicy

        self.settings = settings
        from nous.model_roles import register as register_model_roles
        from nous.platform.models.roles import RoleRegistry

        roles = RoleRegistry()
        register_model_roles(roles)
        from nous.platform.configuration.namespace_policy import entries as namespace_entries
        from nous.platform.configuration.service import ConfigurationValues
        from nous.platform.persistence.namespace_limits import NamespaceLimits

        values = ConfigurationValues(
            {
                **namespace_entries(),
                "models": ConfigurationEntry(
                    initial_resources(),
                    instance_management=True,
                    validator=roles.validate_resources,
                    schema=roles.resources_schema,
                ),
                "contexts": ConfigurationEntry(ContextsPolicy()),
                "feedback": ConfigurationEntry(FeedbackPolicy()),
                "network": ConfigurationEntry(NetworkPolicy(), instance_management=True),
                "retrieval": ConfigurationEntry(retrieval_defaults()),
                "wave": ConfigurationEntry(WavePolicy()),
                "vector_projection": ConfigurationEntry(VectorProjectionPolicy()),
                "growth": ConfigurationEntry(GrowthPolicy()),
                "media": ConfigurationEntry(
                    MediaPolicy(), instance_management=True, scopes=("instance",)
                ),
            }
        )
        self.namespace_limits = NamespaceLimits(values)
        self.authority = Authority(settings.database, namespace_limits=self.namespace_limits)

        self.credentials = CredentialsService(self.authority, settings.token_file)
        self.configuration = ConfigurationService(values, instance=settings.instance)
        self.prompts = PromptService(self.authority)
        self.network = NetworkService(self.configuration)
        self.identity = IdentityService(self.authority)
        from nous.data_management.namespace_backups import NamespaceBackupService
        from nous.platform.content import ContentStore
        from nous.platform.media import MediaService

        self.content = ContentStore(self.authority, settings.instance / "content")
        self.media = MediaService(
            self.content, policy=MediaPolicy(), policy_resolver=self.media_policy
        )
        self.backups = BackupService(self.authority, self.content)
        self.material = MaterialService(self.authority, content_store=self.content)
        from nous.data_management.resources import ResourceService

        self.resources = ResourceService(self.authority, self.material, self.network)
        from nous.material.derived import DerivedService

        self.derivations = DerivedService(self.authority, self.material)
        from nous.imports.media_resolver import MediaInputResolver
        from nous.vector_projection.admission import ProjectionAdmission

        media_inputs = MediaInputResolver(
            self.authority, self.material, self.content, self.media, self.derivations
        )
        self.models = ModelService(
            settings,
            self.prompts,
            self.configuration,
            self.network,
            roles=roles,
            media_resolver=media_inputs.resolve_embedding_input,
            preview_media_resolver=media_inputs.generation_input,
        )
        self.projection_admission = ProjectionAdmission(
            self.authority, self.models, self.configuration
        )
        self.cognition = CognitiveRecords(
            self.authority,
            materials=self.material,
            derivations=self.derivations,
            projection=self.projection_admission,
        )
        from nous.cognition.ego_model import EgoService
        from nous.cognition.entity_changes import EntityService
        from nous.cognition.evolution import EvolutionService
        from nous.cognition.facts import FactService
        from nous.cognition.formation import FormationService
        from nous.cognition.graph_state import GraphService
        from nous.cognition.growth import GrowthService
        from nous.cognition.memory import MemoryService
        from nous.cognition.motivation_records import MotivationService
        from nous.cognition.person_profiles import PersonProfileService
        from nous.cognition.social import SocialService

        self.memories = MemoryService(
            self.authority, self.cognition, materials=self.material, content=self.content
        )
        self.person_profiles = PersonProfileService(
            self.authority, self.cognition, self.models, memories=self.memories
        )
        self.facts = FactService(self.authority, self.cognition, self.person_profiles)
        self.evolution = EvolutionService(self.cognition)
        self.growth = GrowthService(
            self.cognition, evolution=self.evolution, memories=self.memories
        )
        self.entities = EntityService(self.cognition, facts=self.facts)
        self.graphs = GraphService(self.cognition, memories=self.memories)
        self.social = SocialService(self.cognition)
        self.motivation = MotivationService(self.cognition)
        self.ego = EgoService(self.cognition)
        from nous.cognition.corrections import CorrectionService

        self.corrections = CorrectionService(
            self.authority, self.cognition, self.facts, self.models, memories=self.memories
        )
        from nous.references.service import ReferenceDirectory

        self.references = ReferenceDirectory(
            self.authority, self.cognition, self.material, resources=self.resources
        )
        self.execution = DurableExecution(
            self.authority,
            queues=(QueueSpec("default"), QueueSpec("formation"), QueueSpec("embedding")),
        )
        from nous.cognition.episodes import EpisodeService

        self.episodes = EpisodeService(self.authority, self.cognition, self.models, self.execution)
        from nous.context.runtime import ContextRuntime
        from nous.data_management.bundles import BundleService
        from nous.data_management.namespace_archives import NamespaceArchiveService
        from nous.data_management.sources import SourceService
        from nous.imports.media import MediaImportService
        from nous.imports.service import ImportService
        from nous.maintenance import MaintenanceService
        from nous.retrieval.service import RetrievalService

        from nous.vector_projection.service import VectorProjectionService

        self.vector_projection = VectorProjectionService(
            self.authority,
            self.cognition,
            self.models,
            self.configuration,
            materials=self.material,
            derivations=self.derivations,
            admission=self.projection_admission,
        )
        from nous.data_management.dataset_imports import DatasetImportService
        self.retrieval = RetrievalService(
            self.authority,
            self.cognition,
            self.models,
            self.settings.instance / "indexes",
            configuration=self.configuration,
            materials=self.material,
            derivations=self.derivations,
            resources=self.resources,
            projection=self.vector_projection,
        )
        self.formation = FormationService(
            self.cognition, facts=self.facts, configuration=self.configuration,
            memories=self.memories, models=self.models, retrieval=self.retrieval,
            content=self.content,
        )
        self.media_imports = MediaImportService(
            self.authority,
            self.material,
            self.cognition,
            self.models,
            self.content,
            self.media,
            self.derivations,
            formation=self.formation,
            configuration=self.configuration,
        )
        self.imports = ImportService(
            self.authority,
            self.cognition,
            self.models,
            materials=self.material,
            content_store=self.content,
            projection=self.vector_projection,
            media_imports=self.media_imports,
            formation=self.formation,
            configuration=self.configuration,
        )
        from nous.references.reading import ReadingService

        self.maintenance = MaintenanceService(
            self.authority,
            self.cognition,
            self.models,
            self.execution,
            configuration=self.configuration,
            imports=self.imports,
            growth=self.growth,
            retrieval=self.retrieval,
        )
        from nous.cognition.seeds import SeedService

        self.seeds = SeedService(
            self.authority, self.cognition, self.maintenance, self.imports, ego=self.ego,
            materials=self.material, content=self.content
        )
        from nous.retrieval.tuning import RetrievalTuningService

        self.tuning = RetrievalTuningService(
            self.authority,
            cognition=self.cognition,
            material=self.material,
            references=self.references,
            resources=self.resources,
            configuration=self.configuration,
            execution=self.execution,
            retrieval=self.retrieval,
        )
        from nous.vector_projection.spaces import VectorSpaceService

        self.vector_spaces = VectorSpaceService(
            self.authority, self.cognition, self.models, self.configuration, self.vector_projection
        )
        from nous.diagnostics import NamespaceDiagnosticsService

        self.diagnostics = NamespaceDiagnosticsService(
            self.authority,
            self.cognition,
            self.execution,
            self.models,
            self.retrieval,
            self.imports,
        )
        from nous.feedback.service import FeedbackService

        self.feedback = FeedbackService(
            self.authority,
            self.cognition,
            self.configuration,
            self.execution,
            self.models,
            self.corrections,
        )
        from nous.symbols.service import SymbolsService

        self.symbols = SymbolsService(
            self.authority,
            self.cognition,
            self.models,
            self.material,
            self.derivations,
            self.retrieval,
        )
        self.reading = ReadingService(
            self.references, self.retrieval, self.execution, self.maintenance
        )
        self.runtime = ContextRuntime(
            self.authority,
            self.cognition,
            retrieval=self.retrieval,
            materials=self.material,
            content=self.content,
            imports=self.imports,
            configuration=self.configuration,
            models=self.models,
            execution=self.execution,
            derivations=self.derivations,
            feedback_owner=self.feedback,
            symbols=self.symbols,
            motivation=self.motivation,
            reading=self.reading,
        )
        from nous.data_management.lifecycle import DataLifecycleService

        self.lifecycle = DataLifecycleService(
            authority=self.authority,
            contexts=self.runtime,
            vector_projection=self.vector_projection,
            derivations=self.derivations,
            maintenance=self.maintenance,
            corrections=self.corrections,
            resources=self.resources,
            tuning=self.tuning,
            motivation=self.motivation,
            feedback=self.feedback,
        )
        self.sources = SourceService(
            self.authority,
            self.cognition,
            content_store=self.content,
            lifecycle=self.lifecycle,
            memories=self.memories,
        )
        from nous.data_management.namespace_data import NamespaceDataService

        self.namespace_backups = NamespaceBackupService(
            self.authority,
            self.identity,
            self.content,
            settings.instance / "namespace-backups",
            execution=self.execution,
            configuration=self.configuration,
        )
        self.bundles = BundleService(
            self.authority,
            self.cognition,
            content_store=self.content,
            sources=self.sources,
        )
        self.namespace_archives = NamespaceArchiveService(
            self.authority, self.identity, self.bundles, self.configuration
        )
        self.datasets = DatasetImportService(
            self.authority,
            self.cognition,
            self.imports,
            self.bundles,
            self.material,
            self.content,
            memories=self.memories,
        )
        self.namespace_data = NamespaceDataService(
            self.authority,
            self.identity,
            self.content,
            self.execution,
            self.sources,
            self.namespace_backups,
        )

    async def start(self) -> None:
        self.execution.register(
            "nous.cognition.purge_dependents", self.lifecycle.run_cognitive_purge, preemptible=True
        )
        self.execution.register("nous.retrieval.index", self.rebuild_index, preemptible=True)
        self.execution.register("nous.formation.contacts", self.formation.run_contacts, preemptible=True)
        self.execution.register("nous.imports.process", self.imports.run_process, preemptible=True)
        self.execution.register("nous.imports.dataset", self.datasets.run, preemptible=True)
        self.execution.register(
            "nous.imports.temporal_backfill", self.datasets.run_time_backfill, preemptible=True
        )
        self.execution.register("nous.feedback.evaluate", self.feedback.run, preemptible=True)
        self.execution.register("nous.sources.trash", self.sources.run_trash, preemptible=True)
        self.execution.register("nous.retrieval.tune", self.tuning.run, preemptible=True)
        self.execution.register(
            "nous.vectors.project", self.vector_projection.run, preemptible=True
        )
        self.execution.register(
            "nous.person_profile.refresh", self.person_profiles.run_refresh, preemptible=True
        )
        self.execution.register(
            "nous.episodes.rebuild", self.episodes.run_rebuild, preemptible=True
        )
        self.execution.register("nous.imports.text", self.imports.run_text_import, preemptible=True)
        self.execution.register(
            "nous.imports.form_observation", self.imports.run_form_observation, preemptible=True
        )
        self.execution.register(
            "nous.context.resident_tick", self.runtime.resident_tick, preemptible=True
        )
        self.execution.register(
            "nous.context.maintain", self.runtime.run_maintenance, preemptible=True
        )
        self.execution.register(
            "nous.maintenance.generate", self.maintenance.run_generate, preemptible=True
        )
        self.execution.register(
            "nous.maintenance.tick", self.maintenance.run_tick, preemptible=True
        )
        await self.content.initialize()
        await self.authority.initialize()
        await self.configuration.initialize()
        await self.execution.start()
        await self.maintenance.reconcile_schedules()

    async def close(self) -> None:
        failures: list[BaseException] = []
        closers: list[Callable[[], Awaitable[None]]] = [self.execution.close]
        if hasattr(self, "retrieval"):
            closers.append(self.retrieval.close)
        closers.extend((self.network.close, self.authority.close))
        for close in closers:
            try:
                await close()
            except BaseException as error:
                failures.append(error)
        if failures:
            raise BaseExceptionGroup("Application resource shutdown failed", failures)

    async def media_policy(self) -> MediaPolicy:
        effective = await self.configuration.effective_instance("media")
        return MediaPolicy.model_validate(effective["value"])

    async def rebuild_index(self, namespace_id: str, through_commit_seq: int) -> dict[str, object]:
        return await self.retrieval.rebuild_namespace(namespace_id, through_commit_seq)
