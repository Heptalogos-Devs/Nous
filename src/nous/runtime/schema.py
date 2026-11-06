"""Business table declarations collected by the instance composition root."""

from nous.cognition.concept_schema import metadata as concept_metadata
from nous.cognition.feedback_schema import metadata as feedback_metadata
from nous.cognition.schema import metadata as cognition_metadata
from nous.content.schema import metadata as content_metadata
from nous.models.embedding_schema import metadata as embedding_metadata
from nous.storage.content_schema import metadata as blob_metadata
from nous.wave.schema import metadata as wave_metadata

BUSINESS_METADATA = (
    blob_metadata,
    content_metadata,
    embedding_metadata,
    wave_metadata,
    cognition_metadata,
    concept_metadata,
    feedback_metadata,
)
