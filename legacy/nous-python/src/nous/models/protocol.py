"""SDK content encoding for the configured OpenAI-compatible chat contract."""

from typing import cast

from openai.types.chat import ChatCompletionContentPartParam
from pydantic_ai.messages import BinaryContent
from pydantic_ai.models.openai import OpenAIChatModel


class CompatibleChatModel(OpenAIChatModel):
    """Add the video_url content extension while retaining the SDK request lifecycle."""

    async def _map_binary_content_item(self, item: BinaryContent) -> ChatCompletionContentPartParam:
        if item.is_video:
            # OpenAI's typed union does not include the compatible video_url extension.
            return cast(
                ChatCompletionContentPartParam,
                {
                    "type": "video_url",
                    "video_url": {"url": item.data_uri},
                },
            )
        return await super()._map_binary_content_item(item)
