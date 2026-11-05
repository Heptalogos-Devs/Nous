from pydantic import Field

from nous.configuration.metadata import ConfigModel, setting


class TextCacheConfig(ConfigModel):
    max_bytes: int = setting(
        33554432, ge=0, description="Maximum bytes retained for parsed text structures."
    )
    persist: bool = setting(True, description="Persist disposable parsed text structures.")


class ContentConfig(ConfigModel):
    text_cache: TextCacheConfig = Field(default_factory=TextCacheConfig)
    io_chunk_bytes: int = setting(
        1048576, ge=4096, apply="restart", description="Streaming content I/O block size in bytes."
    )
    text_read_max_bytes: int = setting(
        8388608, gt=0, description="Maximum bytes for a convenience UTF-8 text read."
    )
