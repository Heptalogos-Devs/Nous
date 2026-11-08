"""Client export retains exclusive creation on filesystems without hard links."""

import errno
import hashlib
import os
from pathlib import Path
from unittest.mock import patch

import pytest

from nous.transports.cli.control_files import ClientFiles


def test_export_without_hardlink_and_existing_target(tmp_path: Path) -> None:
    source, target = tmp_path / "source", tmp_path / "target"
    source.write_bytes(b"client export")
    files = ClientFiles()
    request: dict[str, object] = {
        "kind": "export_prepare",
        "key": "one",
        "source": str(source),
        "target": str(target),
        "temporary": str(tmp_path / ".nous-export-temp"),
        "digest": hashlib.sha256(source.read_bytes()).hexdigest(),
        "length": source.stat().st_size,
    }
    try:
        files.perform(request)
        with patch("os.link", side_effect=OSError(errno.EOPNOTSUPP, "unsupported")):
            files.perform({"kind": "export_commit", "key": "one"})
        assert target.read_bytes() == source.read_bytes()
        if os.name != "nt":
            assert target.stat().st_mode & 0o777 == source.stat().st_mode & 0o777
        files.perform({**request, "key": "two"})
        with patch("os.link", side_effect=OSError(errno.EOPNOTSUPP, "unsupported")):
            with pytest.raises(FileExistsError):
                files.perform({"kind": "export_commit", "key": "two"})
        assert target.read_bytes() == source.read_bytes()
        linked = tmp_path / "linked"
        files.perform({**request, "key": "three", "target": str(linked)})
        files.perform({"kind": "export_commit", "key": "three"})
        assert linked.read_bytes() == source.read_bytes()
        if os.name != "nt":
            assert linked.stat().st_mode & 0o777 == source.stat().st_mode & 0o777
        assert not list(tmp_path.glob(".nous-export-*"))
    finally:
        files.cleanup()


@pytest.mark.skipif(os.name == "nt", reason="POSIX named pipe is required")
def test_disconnected_client_discards_an_active_export(tmp_path: Path) -> None:
    import threading
    import time

    source, target = tmp_path / "source.fifo", tmp_path / "target"
    os.mkfifo(source)
    files = ClientFiles()
    errors: list[BaseException] = []

    def prepare() -> None:
        try:
            files.perform(
                {
                    "kind": "export_prepare",
                    "key": "one",
                    "source": str(source),
                    "target": str(target),
                    "temporary": str(tmp_path / ".nous-export-active"),
                    "digest": hashlib.sha256(b"x").hexdigest(),
                    "length": 1,
                }
            )
        except BaseException as error:
            errors.append(error)

    worker = threading.Thread(target=prepare, daemon=True)
    worker.start()
    deadline = time.monotonic() + 5
    while not files._active:  # pyright: ignore[reportPrivateUsage]
        assert time.monotonic() < deadline
        time.sleep(0.01)
    files.cleanup()
    assert not list(tmp_path.glob(".nous-export-*"))
    while True:
        try:
            descriptor = os.open(source, os.O_WRONLY | os.O_NONBLOCK)
            break
        except OSError as error:
            assert error.errno == errno.ENXIO and time.monotonic() < deadline
            time.sleep(0.01)
    os.write(descriptor, b"x")
    os.close(descriptor)
    worker.join(timeout=5)
    assert not worker.is_alive() and errors
    assert not target.exists()
