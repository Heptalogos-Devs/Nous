"""Media interpretation and cognitive adoption using authorized prepared inputs."""

from nous.cognition.formation import FormationService
from nous.cognition.records import CognitiveRecords
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.content import ContentStore
from nous.platform.media import MediaService
from nous.platform.models import ModelService
from nous.platform.persistence import Authority

from .media_derivation import derive
from .media_formation import adopt
from .media_resolver import MediaInputResolver


class MediaImportService(MediaInputResolver):
    adopt = adopt
    derive = derive

    def __init__(
        self,
        authority: Authority,
        materials: MaterialService,
        cognition: CognitiveRecords,
        models: ModelService,
        content: ContentStore,
        media: MediaService,
        derivations: DerivedService | None = None,
        *,
        formation: FormationService,
        configuration: ConfigurationService,
    ):
        self.configuration = configuration
        self.formation = formation
        super().__init__(
            authority,
            materials,
            content,
            media,
            derivations or DerivedService(authority, materials),
        )
        self.cognition, self.models = (cognition, models)
