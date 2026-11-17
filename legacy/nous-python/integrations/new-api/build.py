"""Build the optional New API transcription bridge from a pinned official release."""

from __future__ import annotations

import argparse
import os
import subprocess
import tempfile
from pathlib import Path

UPSTREAM = "https://github.com/QuantumNous/new-api.git"
COMMIT = "e835b1bfc43b565d625537830dbfb19328925134"
TAG = "v1.0.0-rc.47"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="nous-new-api:rc47-audio-transcription")
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    integration = Path(__file__).resolve().parent
    repository = integration.parents[1]
    workspace_temp = repository / "temp"
    if workspace_temp.is_symlink() or workspace_temp.is_junction():
        raise SystemExit("Repository temp must be a local directory")
    build_root = workspace_temp / "new-api-audio-build"
    build_root.mkdir(parents=True, exist_ok=True)
    if not build_root.resolve().is_relative_to(workspace_temp.resolve()):
        raise SystemExit("Build directory must resolve inside repository temp")
    temporary = tempfile.TemporaryDirectory(prefix="source-", dir=build_root, delete=False)
    source = Path(temporary.name)
    owned_path = source.resolve()
    expected_parent = build_root.resolve()
    try:
        subprocess.run(
            ["git", "clone", "--depth", "1", "--branch", TAG, UPSTREAM, str(source)], check=True
        )
        actual = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=source, text=True
        ).strip()
        if actual != COMMIT:
            raise SystemExit(f"Build source must be official {TAG} ({COMMIT}); found {actual}")
        if subprocess.check_output(["git", "status", "--porcelain"], cwd=source, text=True).strip():
            raise SystemExit(f"Fresh build source has unexpected changes: {source}")
        patch = integration / "patches" / "0001-audio-transcription-chat.patch"
        subprocess.run(["git", "apply", "--check", str(patch)], cwd=source, check=True)
        subprocess.run(["git", "apply", str(patch)], cwd=source, check=True)
        # Match the official release workflow before Dockerfile embeds the version in UI and Go.
        (source / "VERSION").write_text(TAG + "\n", encoding="utf-8", newline="\n")
        if args.prepare_only:
            print(source)
            return
        environment = dict(os.environ, DOCKER_BUILDKIT="1")
        subprocess.run(
            ["docker", "build", "--tag", args.image, str(source)], env=environment, check=True
        )
        print(args.image)
    finally:
        if not args.prepare_only:
            if source.is_symlink() or source.is_junction() or source.resolve() != owned_path:
                raise RuntimeError(f"Build source changed identity; cleanup refused: {source}")
            if owned_path.parent != expected_parent or not owned_path.is_relative_to(
                repository.resolve() / "temp"
            ):
                raise RuntimeError(
                    f"Build source escaped repository temp; cleanup refused: {source}"
                )
            temporary.cleanup()


if __name__ == "__main__":
    main()
