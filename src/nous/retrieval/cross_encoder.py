"""Optional local Sentence Transformers reranking over bounded text pairs."""

from __future__ import annotations

import importlib
from pathlib import Path
from typing import Protocol, cast

import numpy as np
from nous_sdk.contracts import DomainError

from nous.platform.concurrency import owned_thread


class Encoder(Protocol):
    def predict(self, sentences: list[tuple[str, str]], *, show_progress_bar: bool) -> object: ...


class EncoderFactory(Protocol):
    def __call__(
        self, model_name_or_path: str, *, local_files_only: bool, trust_remote_code: bool
    ) -> Encoder: ...


async def score_pairs(model_path: str | None, query: str, documents: list[str]) -> list[float]:
    if model_path is None:
        raise DomainError(
            "reranker_unavailable", "A local cross encoder model directory is required"
        )

    def predict() -> list[float]:
        if not Path(model_path).is_dir():
            raise DomainError(
                "reranker_unavailable", "A local cross encoder model directory is required"
            )
        try:
            module = importlib.import_module("sentence_transformers")
        except ImportError as error:
            raise DomainError(
                "reranker_unavailable", "Install nous-server[rerank] to use a local cross encoder"
            ) from error
        factory = cast(EncoderFactory, module.CrossEncoder)
        try:
            model = factory(model_path, local_files_only=True, trust_remote_code=False)
            scores = np.asarray(
                model.predict([(query, text) for text in documents], show_progress_bar=False),
                dtype=np.float64,
            )
        except (OSError, ValueError, RuntimeError) as error:
            raise DomainError(
                "reranker_unavailable",
                "The configured local cross encoder could not run",
                reason=str(error),
            ) from error
        if scores.shape != (len(documents),) or not np.isfinite(scores).all():
            raise DomainError(
                "rerank_output_invalid", "Cross encoder must return one finite score per candidate"
            )
        return [float(value) for value in scores]

    return await owned_thread(predict)
