"""Explicit resource composition; domain rules stay with their owners."""

from nous_wave.amemorix.contracts import AMemorixPolicy
from nous_wave.contracts.context import ContextsPolicy

from nous.identity.credentials import CredentialsService
from nous.platform.configuration import Settings
from nous.platform.configuration.models import initial_resources
from nous.platform.configuration.service import ConfigurationService
from nous.platform.execution import DurableExecution, QueueSpec
from nous.platform.models import ModelService
from nous.platform.network.service import NetworkService
from nous.platform.persistence import Authority, BackupService, WriteTx
from nous.platform.prompts import PromptService


class Application:
    def __init__(self, settings: Settings) -> None:
        from nous.cognition.service import CognitionService
        from nous.identity.service import IdentityService
        from nous.material.service import MaterialService
        from nous.retrieval.policy import defaults as retrieval_defaults

        self.settings = settings
        self.authority = Authority(settings.database)
        self.credentials = CredentialsService(self.authority, settings.token_file)
        self.configuration = ConfigurationService(
            self.authority,
            {
                "models": initial_resources(settings.model, settings.embedding).model_dump(
                    mode="json"
                ),
                "integrations.amemorix": AMemorixPolicy().model_dump(mode="json"),
                "contexts": ContextsPolicy().model_dump(mode="json"),
                "network": settings.network.model_dump(mode="json"),
                "retrieval": retrieval_defaults().model_dump(mode="json"),
            },
        )
        self.prompts = PromptService(self.authority)
        self.network = NetworkService(self.configuration)
        self.models = ModelService(
            self.authority, settings, self.prompts, self.configuration, self.network
        )
        from nous.model_roles import register as register_model_roles

        register_model_roles(self.models.roles)
        self.identity = IdentityService(self.authority)
        from nous.platform.content import ContentStore
        from nous.platform.media import MediaService

        self.content = ContentStore(self.authority, settings.instance / "content")
        self.media = MediaService(self.content)
        self.backups = BackupService(self.authority, self.content)
        self.material = MaterialService(self.authority, content_store=self.content)
        from nous.material.derived import DerivedService

        self.derivations = DerivedService(self.authority, self.material)
        self.cognition = CognitionService(
            self.authority, materials=self.material, derivations=self.derivations
        )
        from nous.references.service import ReferenceDirectory

        self.references = ReferenceDirectory(self.authority, self.cognition, self.material)
        self.execution = DurableExecution(
            self.authority,
            queues=(QueueSpec("default"), QueueSpec("formation"), QueueSpec("embedding")),
        )

    async def start(self) -> None:
        from nous.context.service import ContextService
        from nous.data_management.bundles import BundleService
        from nous.data_management.sources import SourceService
        from nous.imports.media import MediaImportService
        from nous.imports.service import ImportService
        from nous.maintenance import MaintenanceService
        from nous.retrieval.service import RetrievalService

        self.bundles = BundleService(
            self.authority, self.cognition, ledger=self.models.ledger, content_store=self.content
        )
        self.media_imports = MediaImportService(
            self.authority,
            self.material,
            self.cognition,
            self.models,
            self.content,
            self.media,
            self.derivations,
        )
        self.imports = ImportService(
            self.authority, self.cognition, self.models, materials=self.material
        )
        self.maintenance = MaintenanceService(
            self.authority, self.cognition, self.models, self.execution
        )
        self.retrieval = RetrievalService(
            self.authority,
            self.cognition,
            self.models,
            self.settings.instance / "indexes",
            configuration=self.configuration,
            materials=self.material,
            derivations=self.derivations,
        )
        self.models.media_resolver = self.media_imports.resolve_embedding_input
        self.models.preview_media_resolver = self.media_imports.generation_input
        self.contexts = ContextService(
            self.authority,
            self.cognition,
            retrieval=self.retrieval,
            materials=self.material,
            configuration=self.configuration,
            models=self.models,
            execution=self.execution,
            derivations=self.derivations,
        )
        self.cognition.invalidate = self.retrieval.invalidate
        self.cognition.purge_context_in = self.contexts.purge_references_in
        self.sources = SourceService(
            self.authority,
            self.cognition,
            purge_in=self.purge_model_inputs_in,
            content_store=self.content,
            invalidate=self.retrieval.invalidate,
            purge_context_in=self.contexts.purge_references_in,
        )
        self.execution.register("nous.imports.process", self.imports.run_process, preemptible=True)
        self.execution.register("nous.imports.text", self.imports.run_text_import, preemptible=True)
        self.execution.register(
            "nous.imports.form_observation", self.imports.run_form_observation, preemptible=True
        )
        self.execution.register("nous.context.prepare", self.contexts.run_prepare, preemptible=True)
        self.execution.register(
            "nous.context.resident_tick", self.contexts.resident_tick, preemptible=True
        )
        self.execution.register(
            "nous.context.maintain", self.contexts.run_maintenance, preemptible=True
        )
        self.execution.register(
            "nous.maintenance.generate", self.maintenance.run_generate, preemptible=True
        )
        self.execution.register(
            "nous.maintenance.tick", self.maintenance.run_tick, preemptible=True
        )
        await self.content.initialize()
        await self.authority.initialize()
        await self.execution.start()
        await self.maintenance.reconcile_schedules()

    async def close(self) -> None:
        await self.execution.close()
        if hasattr(self, "retrieval"):
            await self.retrieval.close()
        await self.network.close()
        await self.authority.close()

    async def purge_model_inputs_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        await self.models.ledger.purge_materials_in(tx, material_ids)
        await self.derivations.purge_materials_in(tx, material_ids)
        await self.maintenance.purge_materials_in(tx, material_ids)
