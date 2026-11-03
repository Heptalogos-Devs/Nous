"""Synthesize speech with the official Volcengine Agent Plan HTTP TTS endpoint."""

from __future__ import annotations

import argparse
import asyncio
import base64
import binascii
import hashlib
import json
import math
import os
import sys
import tempfile
from datetime import UTC, datetime
from pathlib import Path
from typing import cast
from urllib.parse import urlsplit
from uuid import uuid4

import httpx2

DEFAULT_ENDPOINT = "https://openspeech.bytedance.com/api/v3/plan/tts/unidirectional"
REPOSITORY = Path(__file__).resolve().parents[2]
SUFFIX_FORMATS = {
    ".mp3": "mp3",
    ".wav": "wav",
    ".pcm": "pcm",
    ".ogg": "ogg_opus",
    ".opus": "ogg_opus",
}


class SynthesisError(Exception):
    """A failed request or incomplete audio stream, with no automatic resubmission."""


def read_api_key(path: Path | None, environment_name: str | None) -> str:
    value = (
        path.read_text(encoding="utf-8").strip()
        if path
        else os.environ.get(environment_name or "", "").strip()
    )
    if not value:
        raise SynthesisError("The selected API key file or environment variable is empty")
    if "\r" in value or "\n" in value:
        raise SynthesisError("The selected credential must contain exactly one API key")
    return value


def service_message(value: object, api_key: str) -> str:
    """Keep service diagnostics useful while excluding the supplied credential."""
    return str(value).replace(api_key, "[redacted]")[:1500]


async def synthesize(
    *,
    text: str,
    output: Path,
    api_key: str,
    endpoint: str = DEFAULT_ENDPOINT,
    resource: str = "seed-tts-2.0",
    speaker: str = "zh_female_vv_uranus_bigtts",
    audio_format: str | None = None,
    sample_rate: int = 24000,
    speech_rate: int = 0,
    styles: tuple[str, ...] = (),
    timeout_seconds: float = 120,
    trust_env: bool = False,
    overwrite: bool = False,
) -> dict[str, object]:
    """Submit once, close the response on cancellation, and publish only complete audio."""
    text = text.strip()
    if not text:
        raise SynthesisError("Speech text must not be empty")
    if not math.isfinite(timeout_seconds) or timeout_seconds <= 0 or not -50 <= speech_rate <= 100:
        raise SynthesisError("Timeout must be positive and speech rate must be between -50 and 100")
    if sample_rate not in {8000, 16000, 22050, 24000, 32000, 44100, 48000}:
        raise SynthesisError("Unsupported sample rate")
    inferred_format = SUFFIX_FORMATS.get(output.suffix.lower())
    if inferred_format is None:
        raise SynthesisError("Output suffix must be .mp3, .wav, .pcm, .ogg or .opus")
    if audio_format is None:
        audio_format = inferred_format
    elif audio_format != inferred_format:
        raise SynthesisError(
            f"Requested format {audio_format} conflicts with output suffix {output.suffix}"
        )
    if audio_format == "ogg_opus" and sample_rate != 48000:
        raise SynthesisError("OGG Opus requires a 48000 Hz sample rate")
    address = urlsplit(endpoint)
    if address.username or address.password or address.query or address.fragment:
        raise SynthesisError("The endpoint must not contain credentials, a query or a fragment")
    if address.scheme != "https" and not (
        address.scheme == "http" and address.hostname in {"127.0.0.1", "localhost", "::1"}
    ):
        raise SynthesisError("Use HTTPS for the service endpoint")
    workspace_temp = REPOSITORY / "temp"
    if workspace_temp.is_symlink() or workspace_temp.is_junction():
        raise SynthesisError("Repository temp must be a local directory")
    output, resolved_temp = await asyncio.gather(
        asyncio.to_thread(output.resolve), asyncio.to_thread(workspace_temp.resolve)
    )
    if not output.is_relative_to(resolved_temp):
        raise SynthesisError("Generated audio must be inside repository temp")
    transcript_path = output.with_suffix(output.suffix + ".txt")
    metadata_path = output.with_suffix(output.suffix + ".json")
    if not overwrite and any(path.exists() for path in (output, transcript_path, metadata_path)):
        raise SynthesisError("An output already exists; choose another path or use --overwrite")
    output.parent.mkdir(parents=True, exist_ok=True)

    request_id = str(uuid4())
    headers = {
        "X-Api-Key": api_key,
        "X-Api-Resource-Id": resource,
        "X-Api-Request-Id": request_id,
        "X-Control-Require-Usage-Tokens-Return": "*",
        "Content-Type": "application/json",
    }
    parameters: dict[str, object] = {
        "text": text,
        "speaker": speaker,
        "audio_params": {
            "format": audio_format,
            "sample_rate": sample_rate,
            "speech_rate": speech_rate,
        },
    }
    if styles:
        parameters["additions"] = json.dumps({"context_texts": list(styles)}, ensure_ascii=False)
    audio_bytes = 0
    audio_chunks = 0
    usage: dict[str, object] | None = None
    log_id: str | None = None
    completed = False
    digest = hashlib.sha256()
    scratch_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=output.parent, prefix=f".{output.name}.", suffix=".part", delete=False
        ) as scratch:
            scratch_path = Path(scratch.name)
            # HTTPX transport retries default to zero; every run submits exactly one POST.
            async with (
                asyncio.timeout(timeout_seconds),
                httpx2.AsyncClient(
                    timeout=httpx2.Timeout(timeout_seconds, connect=min(10, timeout_seconds)),
                    trust_env=trust_env,
                    follow_redirects=False,
                ) as client,
            ):
                async with client.stream(
                    "POST", endpoint, headers=headers, json={"req_params": parameters}
                ) as response:
                    log_id = response.headers.get("x-tt-logid")
                    if response.status_code != 200:
                        body = (await response.aread()).decode("utf-8", errors="replace")
                        raise SynthesisError(
                            f"TTS HTTP {response.status_code}: {service_message(body, api_key)}; log_id={log_id}"
                        )
                    async for line in response.aiter_lines():
                        if not line.strip():
                            continue
                        decoded: object = json.loads(line)
                        if not isinstance(decoded, dict):
                            raise SynthesisError("TTS returned a non-object stream frame")
                        frame = cast(dict[str, object], decoded)
                        code = frame.get("code")
                        if code not in (0, 20000000):
                            raise SynthesisError(
                                f"TTS code {code}: {service_message(frame.get('message', 'request failed'), api_key)}; log_id={log_id}"
                            )
                        reported = frame.get("usage")
                        if isinstance(reported, dict):
                            # Usage is a cumulative snapshot, not a charge per audio chunk.
                            usage = cast(dict[str, object], reported)
                        data = frame.get("data")
                        if data:
                            if not isinstance(data, str):
                                raise SynthesisError("TTS audio data must be base64 text")
                            chunk = base64.b64decode(data, validate=True)
                            scratch.write(chunk)
                            digest.update(chunk)
                            audio_bytes += len(chunk)
                            audio_chunks += 1
                        if code == 20000000:
                            completed = True
                            break
            if not completed or not audio_bytes:
                raise SynthesisError(f"TTS stream ended without complete audio; log_id={log_id}")
        scratch_path.replace(output)
        scratch_path = None
        transcript_path.write_text(text + "\n", encoding="utf-8", newline="\n")
        metadata: dict[str, object] = {
            "audio": str(output),
            "text_file": str(transcript_path),
            "text": text,
            "endpoint": endpoint,
            "resource_id": resource,
            "speaker": speaker,
            "format": audio_format,
            "sample_rate": sample_rate,
            "speech_rate": speech_rate,
            "styles": list(styles),
            "request_id": request_id,
            "service_log_id": log_id,
            "created_at": datetime.now(UTC).isoformat(),
            "audio_bytes": audio_bytes,
            "audio_chunks": audio_chunks,
            "audio_sha256": digest.hexdigest(),
            "text_sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(),
            "usage": usage,
        }
        metadata_path.write_text(
            json.dumps(metadata, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        return {**metadata, "metadata_file": str(metadata_path)}
    finally:
        if scratch_path is not None:
            scratch_path.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--text")
    source.add_argument("--text-file", type=Path)
    credential = parser.add_mutually_exclusive_group(required=True)
    credential.add_argument("--api-key-file", type=Path)
    credential.add_argument(
        "--api-key-env",
        help="Name of the environment variable containing the upstream Agent Plan key",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--endpoint", default=DEFAULT_ENDPOINT)
    parser.add_argument("--resource", default="seed-tts-2.0")
    parser.add_argument("--speaker", default="zh_female_vv_uranus_bigtts")
    parser.add_argument(
        "--format",
        choices=("mp3", "wav", "pcm", "ogg_opus"),
        help="Inferred from --output suffix when omitted; an explicit format must match the suffix",
    )
    parser.add_argument("--sample-rate", type=int, default=24000)
    parser.add_argument("--speech-rate", type=int, default=0)
    parser.add_argument("--style", action="append", default=[])
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument(
        "--trust-env", action="store_true", help="Use configured transport environment settings"
    )
    parser.add_argument("--overwrite", action="store_true")
    arguments = parser.parse_args()
    key = ""
    try:
        text = (
            arguments.text_file.read_text(encoding="utf-8")
            if arguments.text_file
            else arguments.text
        )
        key = read_api_key(arguments.api_key_file, arguments.api_key_env)
        result = asyncio.run(
            synthesize(
                text=text,
                output=arguments.output,
                api_key=key,
                endpoint=arguments.endpoint,
                resource=arguments.resource,
                speaker=arguments.speaker,
                audio_format=arguments.format,
                sample_rate=arguments.sample_rate,
                speech_rate=arguments.speech_rate,
                styles=tuple(arguments.style),
                timeout_seconds=arguments.timeout,
                trust_env=arguments.trust_env,
                overwrite=arguments.overwrite,
            )
        )
        print(json.dumps(result, ensure_ascii=False))
    except (
        SynthesisError,
        httpx2.HTTPError,
        TimeoutError,
        OSError,
        ValueError,
        binascii.Error,
    ) as error:
        message = service_message(error, key) if key else str(error)
        print(
            json.dumps(
                {"error": {"type": type(error).__name__, "message": message}}, ensure_ascii=False
            ),
            file=sys.stderr,
        )
        raise SystemExit(1) from None
    except KeyboardInterrupt:
        print(
            json.dumps({"error": {"type": "cancelled", "message": "Synthesis cancelled"}}),
            file=sys.stderr,
        )
        raise SystemExit(130) from None


if __name__ == "__main__":
    main()
