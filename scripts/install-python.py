#!/usr/bin/env python3
"""Install one matching server/SDK/native wheel set using uv or Python's pip."""

from __future__ import annotations

import argparse
import email.parser
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

PACKAGES = ("nous-wave-server", "nous-wave", "nous-wave-native")


def data_directory() -> Path:
    if sys.platform == "win32":
        return (
            Path(os.environ.get("LOCALAPPDATA", str(Path.home() / "AppData" / "Local")))
            / "NousWave"
        )
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "NousWave"
    return (
        Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local" / "share"))) / "nous-wave"
    )


def wheel_versions(directory: Path) -> dict[str, dict[str, list[Path]]]:
    versions: dict[str, dict[str, list[Path]]] = {name: {} for name in PACKAGES}
    for path in directory.glob("*.whl"):
        with zipfile.ZipFile(path) as wheel:
            metadata_files = [
                name for name in wheel.namelist() if name.endswith(".dist-info/METADATA")
            ]
            if len(metadata_files) != 1:
                raise ValueError(f"Invalid wheel metadata: {path.name}")
            metadata = email.parser.BytesParser().parsebytes(wheel.read(metadata_files[0]))
            name = re.sub(r"[-_.]+", "-", metadata.get("Name", "")).lower()
            version = metadata.get("Version")
            if name in versions and version:
                versions[name].setdefault(version, []).append(path)
    return versions


def run(arguments: list[str], *, quiet: bool = False) -> None:
    subprocess.run(arguments, check=True, stdout=subprocess.DEVNULL if quiet else None)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--wheels",
        type=Path,
        default=Path(__file__).resolve().parent,
        help="Directory containing all three project wheels (default: beside this script)",
    )
    parser.add_argument(
        "--version", help="Choose a version when the wheel directory contains several releases"
    )
    parser.add_argument("--venv", type=Path, default=data_directory() / "venv")
    parser.add_argument("--instance", type=Path, default=data_directory() / "instance")
    parser.add_argument("--python", default="3.14", help="CPython 3.14 selector for uv")
    parser.add_argument(
        "--offline",
        action="store_true",
        help="Require all dependency wheels in --wheels; disable package indexes",
    )
    args = parser.parse_args()
    directory = args.wheels.resolve()
    versions = wheel_versions(directory)
    common = set(versions[PACKAGES[0]])
    for name in PACKAGES[1:]:
        common.intersection_update(versions[name])
    if args.version:
        if args.version not in common:
            parser.error("The requested release needs server, SDK and native wheels in --wheels")
        version = args.version
    elif len(common) == 1:
        version = common.pop()
    else:
        parser.error("Supply one complete three-wheel release, or choose --version explicitly")
    if any(len(versions[name][version]) != 1 for name in PACKAGES):
        parser.error(
            "Use the matching platform bundle with exactly one wheel per project and version"
        )
    environment = args.venv.resolve()
    instance = args.instance.resolve()
    scripts = environment / ("Scripts" if os.name == "nt" else "bin")
    python = scripts / ("python.exe" if os.name == "nt" else "python")
    nous = scripts / ("nous.exe" if os.name == "nt" else "nous")
    uv = shutil.which("uv")
    if not (environment / "pyvenv.cfg").is_file():
        if environment.exists():
            parser.error(
                "--venv already exists and is not a virtual environment; choose a new directory"
            )
        if uv:
            create = [uv, "venv", "--python", args.python, str(environment)]
            if args.offline:
                create.append("--offline")
            run(create)
        else:
            if sys.version_info[:2] != (3, 14) or platform.python_implementation() != "CPython":
                parser.error("Install uv, or run this script with standard CPython 3.14")
            run([sys.executable, "-m", "venv", str(environment)])
    run(
        [
            str(python),
            "-I",
            "-c",
            "import platform,sys,sysconfig; "
            "valid=platform.python_implementation()=='CPython' and sys.version_info[:2]==(3,14) "
            "and not sysconfig.get_config_var('Py_GIL_DISABLED'); "
            "sys.exit(0 if valid else 'Nous requires standard CPython 3.14 with the GIL')",
        ]
    )
    requirements = [str(versions[name][version][0]) for name in PACKAGES]
    options = ["--only-binary=:all:", "--find-links", str(directory)]
    if args.offline:
        options.append("--no-index")
    if uv:
        install = [uv, "pip", "install", "--python", str(python), *options, *requirements]
        for package in PACKAGES:
            install.extend(["--reinstall-package", package])
        if args.offline:
            install.append("--offline")
        run(install)
    else:
        run([str(python), "-m", "pip", "install", "--force-reinstall", *options, *requirements])
    run(
        [
            str(python),
            "-I",
            "-c",
            "import importlib.metadata as m; "
            "import nous,nous_wave.contracts,nous_native; "
            "expected=__import__('sys').argv[1]; "
            "assert all(m.version(name)==expected for name in "
            "('nous-wave-server','nous-wave','nous-wave-native'))",
            version,
        ]
    )
    run([str(nous), "--help"], quiet=True)
    run([str(nous), "init", "--instance", str(instance)])
    command = [str(nous), "serve", "--instance", str(instance)]
    display = subprocess.list2cmdline(command) if os.name == "nt" else shlex.join(command)
    print(f"Installed Nous Wave {version} into {environment}")
    print(f"Start the service: {display}")
    print("The protected instance credential stays in the instance's credentials/admin.token file.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError, zipfile.BadZipFile) as error:
        raise SystemExit(f"Installation failed: {error}") from error
