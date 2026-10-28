"""Explicit resource composition; domain rules stay with their owners."""

from nous_wave.amemorix.contracts import AMemorixPolicy

from nous.identity.credentials import CredentialsService
from nous.platform.configuration import Settings
from nous.platform.configuration.models import initial_resources
from nous.platform.configuration.service import ConfigurationService
from nous.platform.execution import DurableExecution, QueueSpec
from nous.platform.models import ModelService
from nous.platform.persistence import Authority, BackupService
from nous.platform.prompts import PromptService


class Application:
    def __init__(self, settings: Settings) -> None:
        from nous.cognition.service import CognitionService
        from nous.identity.service import IdentityService
        from nous.material.service import MaterialService

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
            },
        )
        self.prompts = PromptService(self.authority)
        self.models = ModelService(self.authority, settings, self.prompts, self.configuration)
        self.identity = IdentityService(self.authority)
        from nous.platform.content import ContentStore
        from nous.platform.media import MediaService

        self.content = ContentStore(self.authority, settings.instance / "content")
        self.media = MediaService(self.content)
        self.backups = BackupService(self.authority, self.content)
        self.material = MaterialService(self.authority, content_store=self.content)
        self.cognition = CognitionService(self.authority)
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
        from nous.retrieval.service import RetrievalService

        self.bundles = BundleService(self.authority, self.cognition, content_store=self.content)
        self.media_imports = MediaImportService(
            self.authority, self.material, self.cognition, self.models, self.content, self.media
        )
        self.imports = ImportService(
            self.authority, self.cognition, self.models, materials=self.material
        )
        self.retrieval = RetrievalService(
            self.authority, self.cognition, self.models, self.settings.instance / "indexes"
        )
        self.models.media_resolver = self.media_imports.resolve_embedding_input
        self.contexts = ContextService(
            self.authority, self.cognition, retrieval=self.retrieval, materials=self.material
        )
        self.cognition.invalidate = self.retrieval.invalidate
        self.cognition.purge_context_in = self.contexts.purge_references_in
        self.sources = SourceService(
            self.authority,
            self.cognition,
            purge_in=self.models.ledger.purge_materials_in,
            content_store=self.content,
            invalidate=self.retrieval.invalidate,
            purge_context_in=self.contexts.purge_references_in,
        )
        self.execution.register("nous.imports.process", self.imports.run_process, preemptible=True)
        await self.content.initialize()
        await self.authority.initialize()
        await self.execution.start()

    async def close(self) -> None:
        await self.execution.close()
        if hasattr(self, "retrieval"):
            await self.retrieval.close()
        await self.models.close()
        await self.authority.close()
