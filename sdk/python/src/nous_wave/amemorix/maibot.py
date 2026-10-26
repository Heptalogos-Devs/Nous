import os

from ..client import NousClient
from .client import AMemorixAdapter


def connect_from_environment() -> AMemorixAdapter:
    client = NousClient(
        os.environ["NOUS_URL"], credential_file=os.environ["NOUS_TOKEN_FILE"]
    )
    return AMemorixAdapter(
        client,
        namespace_id=os.environ["NOUS_NAMESPACE_ID"],
        subject_id=os.environ["NOUS_SUBJECT_ID"],
    )
