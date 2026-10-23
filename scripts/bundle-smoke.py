"""Exercise offline bundle inspection through the real Nous CLI.

Usage: python scripts/bundle-smoke.py <nous-binary>
Uses the upstream-exported knowledge v1, full v1 and image v2 fixtures. The
copied executable, inputs, command output and report stay under project temp/.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import uuid
import zipfile

sys.dont_write_bytecode = True


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def default_instance(environment):
    # Match directories::ProjectDirs("org", "Heptalogos", "NousWave") used by
    # InstancePaths. This only resolves the path; it creates and reads no secrets.
    if os.name == "nt":
        import ctypes
        buffer = ctypes.create_unicode_buffer(32768)
        result = ctypes.windll.shell32.SHGetFolderPathW(None, 0x001C, None, 0, buffer)
        require(result == 0, "cannot resolve Windows LocalAppData for the side-effect check")
        return Path(buffer.value) / "Heptalogos" / "NousWave" / "data" / "default"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "org.Heptalogos.NousWave" / "default"
    configured = Path(environment.get("XDG_DATA_HOME", ""))
    base = configured if configured.is_absolute() else Path.home() / ".local" / "share"
    return base / "nouswave" / "default"


def tree_metadata(path):
    if not path.exists():
        return None
    entries = {}
    for entry in [path, *sorted(path.rglob("*"))]:
        state = entry.lstat()
        entries[str(entry.relative_to(path))] = [state.st_mode, state.st_size, state.st_mtime_ns]
    return entries


class Runner:
    def __init__(self, binary, project):
        run_id = time.strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex[:8]
        self.root = project / "temp" / "runtime" / "bundle-smoke" / run_id
        self.work = self.root / "work"
        self.inputs = self.root / "inputs"
        self.logs = self.root / "commands"
        for directory in (self.work, self.inputs, self.logs, self.root / "tmp"):
            directory.mkdir(parents=True, exist_ok=True)
        self.binary = self.root / binary.name
        shutil.copy2(binary, self.binary)
        self.environment = dict(os.environ)
        for name in ("NOUS_INSTANCE", "NOUS_URL", "NOUS_TOKEN_FILE", "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"):
            self.environment.pop(name, None)
        for name in ("TEMP", "TMP", "TMPDIR"):
            self.environment[name] = str(self.root / "tmp")
        self.environment["PYTHONDONTWRITEBYTECODE"] = "1"
        self.flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
        self.report = {"run_id": run_id, "runtime": str(self.root), "checks": {}, "commands": []}

    def inspect(self, path, *, details=False, succeeds=True):
        command = [str(self.binary), "import", "bundle", "inspect", "--file", str(path)]
        if details:
            command.append("--details")
        result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8",
                                cwd=self.work, env=self.environment, creationflags=self.flags, timeout=40)
        number = len(self.report["commands"]) + 1
        stdout = self.logs / f"{number:02d}.stdout.json"
        stderr = self.logs / f"{number:02d}.stderr.txt"
        stdout.write_text(result.stdout, encoding="utf-8")
        stderr.write_text(result.stderr, encoding="utf-8")
        self.report["commands"].append({"file": path.name, "details": details,
                                        "returncode": result.returncode,
                                        "stdout": str(stdout), "stderr": str(stderr)})
        if succeeds:
            require(result.returncode == 0, f"inspect {path.name} failed: {result.stderr.strip()}")
            return json.loads(result.stdout)
        require(result.returncode != 0, f"invalid bundle {path.name} was accepted")
        return result

    def save(self, error=None):
        self.report["passed"] = error is None
        if error is not None:
            self.report["error"] = str(error)
        path = self.root / "report.json"
        path.write_text(json.dumps(self.report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        return path


def inspect_fixture(runner, source, provenance):
    path = runner.inputs / source.name
    shutil.copy2(source, path)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    require(digest == provenance["sha256"], f"fixture provenance hash differs: {source.name}")
    with zipfile.ZipFile(path) as archive:
        manifest = json.loads(archive.read("manifest.json"))
        knowledge = json.loads(archive.read("knowledge.json"))
        state = json.loads(archive.read("state.json")) if "state.json" in archive.namelist() else None
        images = json.loads(archive.read("images.json")) if "images.json" in archive.namelist() else None
        summary = runner.inspect(path)
        require(summary["status"] == "validated" and summary["manifest"] == manifest,
                f"validated summary lost its actual manifest: {source.name}")
        require(summary["archive"]["sha256"] == digest and summary["archive"]["bytes"] == path.stat().st_size,
                f"summary identifies a different archive: {source.name}")
        details = runner.inspect(path, details=True)
        require(details["knowledge"] == knowledge, f"details changed original knowledge structure: {source.name}")
        require(details["state"] == state, f"details changed full state tables: {source.name}")
        require(details["images"] == images, f"details changed image provenance: {source.name}")
        require(set(details["members"]) == set(archive.namelist()), f"details omitted archive members: {source.name}")
        vector_paths = {name for name in archive.namelist() if name.endswith(".npz")}
        require(set(details["index"]["vector_members"].values()) == vector_paths,
                f"details omitted NPZ inventory: {source.name}")
        require(summary["contents"]["vector_members"] == details["index"]["vector_members"],
                f"summary and details disagree on NPZ inventory: {source.name}")
        assets = images["assets"] if images else []
        for name in vector_paths | {asset["member"] for asset in assets}:
            payload = archive.read(name)
            member = details["members"][name]
            require(member["sha256"] == hashlib.sha256(payload).hexdigest() and member["byte_len"] == len(payload),
                    f"binary member inventory changed: {name}")
        require(summary["contents"]["documents"] == len(knowledge["docs"])
                and summary["contents"]["image_assets"] == len(assets),
                f"summary lost content counts: {source.name}")
        capabilities = {item["capability"] for item in summary["required_capabilities"]}
        require(summary["required_capabilities"] == details["required_capabilities"],
                f"summary omitted required capabilities: {source.name}")
        if state:
            require({"graph", "episodes", "facts", "profiles"} <= capabilities,
                    f"full bundle capability requirements are incomplete: {source.name}")
        if images:
            require({"images", "image_embeddings"} <= capabilities,
                    "image bundle capability requirements are incomplete")
    label = source.stem
    runner.report["checks"][label + ":validated"] = True
    runner.report["checks"][label + ":details_preserved"] = True
    return path


def corrupt_copies(source, directory):
    checksum = directory / "invalid-checksum.amembundle"
    illegal_path = directory / "invalid-path.amembundle"
    with zipfile.ZipFile(source) as original:
        for destination in (checksum, illegal_path):
            with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as output:
                for member in original.infolist():
                    payload = original.read(member)
                    if destination == checksum and member.filename == "knowledge.json":
                        payload += b"\n"  # Valid JSON, same semantics, stale member checksum.
                    output.writestr(copy.copy(member), payload)
                if destination == illegal_path:
                    output.writestr("../outside.txt", b"forbidden archive member")
    return checksum, illegal_path


def exercise(runner, project):
    fixture_root = project / "crates" / "nous-app" / "tests" / "fixtures" / "bundles"
    sources = json.loads((fixture_root / "sources.json").read_text(encoding="utf-8"))
    require(sources["commit"] == "c602a2780f4f4dda4a6be46d67abc1cfb1ad420c", "unexpected upstream fixture commit")
    indexed = {item["file"]: item for item in sources["fixtures"]}
    default = default_instance(runner.environment)
    before = tree_metadata(default)
    runner.report.update(fixture_source=sources["source"], fixture_commit=sources["commit"], default_instance=str(default))
    try:
        paths = []
        for name in ("v1-knowledge-1.0.0.amembundle", "v1-full-1.0.0.amembundle", "v2-images-1.0.0.amembundle"):
            paths.append(inspect_fixture(runner, fixture_root / name, indexed[name]))
        checksum, illegal_path = corrupt_copies(paths[0], runner.inputs)
        failed = runner.inspect(checksum, succeeds=False)
        require("checksum" in failed.stderr.lower(), "checksum fixture failed outside checksum validation")
        runner.report["checks"]["invalid_checksum_rejected"] = True
        runner.inspect(illegal_path, succeeds=False)
        runner.report["checks"]["invalid_archive_path_rejected"] = True
    finally:
        require(tree_metadata(default) == before, "offline inspection changed the default instance directory")
        require(not list(runner.work.iterdir()), "offline inspection created files in its working directory")
    runner.report["checks"]["without_instance_or_credentials"] = True
    runner.report["checks"]["no_default_instance_files"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    arguments = parser.parse_args()
    project = Path(__file__).resolve().parents[1]
    runner = Runner(arguments.binary.resolve(strict=True), project)
    try:
        exercise(runner, project)
    except Exception as error:
        report = runner.save(error)
        raise SystemExit(f"Bundle smoke failed: {error}\nReport: {report}") from None
    report = runner.save()
    print(json.dumps({"passed": True, "checks": runner.report["checks"], "report": str(report)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
