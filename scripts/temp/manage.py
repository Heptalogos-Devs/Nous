from __future__ import annotations

import argparse
import contextlib
import hashlib
import os
import re
import shutil
import stat
import sys
import uuid
from collections.abc import Generator, Sequence
from pathlib import Path

from process_activity import block_reason, busy_paths
from shared import Unsafe, command, within

STANDARD = (
    "build",
    "cache",
    "runtime",
    "work",
    "reference-code",
    "tools",
    "venvs",
    "test-materials",
)
MATERIAL_DIRS = (
    "Audio/Speech/TTS",
    "Audio/Speech/FormatChecks",
    "Audio/Music",
    "Image/Scenes",
    "Image/Text",
    "Image/Illustration",
    "Image/Pixel",
    "Image/Screenshots",
    "Image/Photography",
    "Image/Probes",
    "PDF",
    "Text",
    "Video",
    "Instances",
    "Config",
    "Prompts",
    "Schemas",
    "Collections",
    "Results/Image",
    "Results/Audio/ASR",
    "Results/Audio/Music",
    "Exports",
    "Diagnostics/Fixtures",
    "Incoming",
)
MEDIA = set(
    (
        ".pdf .png .jpg .jpeg .gif .webp .bmp .tif .tiff .avif .heic .svg "
        ".wav .mp3 .flac .ogg .opus .m4a .aac .aiff .aif .wma .amr .pcm "
        ".mp4 .mov .mkv .webm .avi .m4v .mpg .mpeg "
        ".doc .docx .ppt .pptx .xls .xlsx .epub .srt .vtt"
    ).split()
)
EXCLUDED_TOP = re.compile(
    r"^(build|cache|dist|runtime|work|reference-code|wheel-installed|"
    r"sdist-check|tools|venvs|new-api.*|native-.*|nous-.*|pytest-.*)$",
    re.I,
)
EXCLUDED_PARTS = {
    ".git",
    "node_modules",
    "cache",
    "__pycache__",
    "target",
    "site-packages",
    "content",
    "index",
    "credentials",
}
ROOT = Path(__file__).resolve().parents[2]
TEMP = ROOT / "temp"
MATERIALS = TEMP / "test-materials"
LOCK = ".cleanup.lock"
DIR_FLAGS = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)


Fingerprint = tuple[int, int, int, int, int, int]
Identity = tuple[int, int]
Snapshot = dict[Path, Fingerprint]


@contextlib.contextmanager
def directory(path: Path, create: bool = False) -> Generator[int, None, None]:  # noqa: UP043
    """Walk from / using directory handles; never traverse a symbolic link."""
    fd = os.open("/", DIR_FLAGS)
    try:
        for part in Path(path).parts[1:]:
            if create:
                try:
                    os.mkdir(part, dir_fd=fd)
                except FileExistsError:
                    pass
            next_fd = os.open(part, DIR_FLAGS, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        yield fd
    finally:
        os.close(fd)


def fingerprint(info: os.stat_result) -> Fingerprint:
    return (
        info.st_dev,
        info.st_ino,
        info.st_mode,
        info.st_size,
        info.st_mtime_ns,
        info.st_ctime_ns,
    )


def identity(info: os.stat_result) -> Identity:
    return info.st_dev, info.st_ino


def initialize() -> None:
    if os.name == "nt":
        return windows_initialize()
    paths = [TEMP / name for name in STANDARD]
    paths += [MATERIALS / name for name in MATERIAL_DIRS]
    # Validate every existing ancestor before making any directories.
    for path in paths:
        for ancestor in (path, *path.parents):
            try:
                info = ancestor.lstat()
            except FileNotFoundError:
                continue
            if not stat.S_ISDIR(info.st_mode):
                raise Unsafe(f"标准目录或其祖先不是普通目录：{ancestor}")
    for path in paths:
        with directory(path, create=True):
            pass
    print("标准 temp 目录已就绪。")


def initialize_agents() -> None:
    if os.name == "nt":
        return windows_initialize_agents()
    with directory(TEMP) as fd:
        try:
            os.stat("AGENTS.md", dir_fd=fd, follow_symlinks=False)
            return
        except FileNotFoundError:
            pass
        template = (Path(__file__).parent / "res/AGENTS.template.md").read_text(encoding="utf-8")
        content = template.replace("<ROOT>", "..").encode("utf-8")
        target = os.open(
            "AGENTS.md", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o666, dir_fd=fd
        )
        with os.fdopen(target, "wb") as output:
            output.write(content)
    print("已从模板创建 temp/AGENTS.md。")


@contextlib.contextmanager
def cleanup_lock() -> Generator[Identity, None, None]:  # noqa: UP043
    if os.name == "nt":
        with windows_cleanup_lock() as root_id:
            yield root_id
        return
    import fcntl

    with directory(TEMP) as root_fd:
        fd = os.open(LOCK, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600, dir_fd=root_fd)
        try:
            if not stat.S_ISREG(os.fstat(fd).st_mode):
                raise Unsafe("清理锁不是普通文件。")
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise Unsafe("另一个 temp 清理正在运行；本次未清理。") from None
            yield identity(os.fstat(root_fd))
        finally:
            os.close(fd)


def is_material(path: Path, top: str) -> bool:
    if EXCLUDED_TOP.fullmatch(top):
        return False
    if any(part.lower() in EXCLUDED_PARTS for part in path.relative_to(TEMP).parts):
        return False
    ext = path.suffix.lower()
    if ext in MEDIA:
        return True
    if top.lower() in (
        "media-verification",
        "music-samples",
        "volcengine-tts",
        "volcengine-tts-check",
    ):
        return ext in (".json", ".txt", ".md", ".html", ".ffconcat", ".ssml", ".bin")
    return (
        ext == ".md"
        or path.name.lower() == "custom-array-schema.json"
        or (
            ext == ".txt"
            and re.search(r"material|sample|transcript|script", path.stem, re.I) is not None
        )
    )


def snapshot(path: Path) -> Snapshot:
    """Do not follow links; reject mount points and linked worktrees; capture changes."""
    if os.name == "nt":
        return windows_snapshot(path)
    result: Snapshot = {}
    pending = [path]
    while pending:
        entry = pending.pop()
        info = entry.lstat()
        if stat.S_ISLNK(info.st_mode):
            if entry == path or entry.name == ".git":
                raise Unsafe("候选或 Git 入口为符号链接，保留")
            result[entry.relative_to(path)] = fingerprint(info)
            continue
        if not stat.S_ISDIR(info.st_mode) and not stat.S_ISREG(info.st_mode):
            raise Unsafe("子树含特殊文件，保留")
        if os.path.ismount(entry):
            raise Unsafe("子树含挂载点，保留")
        result[entry.relative_to(path)] = fingerprint(info)
        if stat.S_ISDIR(info.st_mode):
            with directory(entry) as fd:
                pending.extend(entry / name for name in os.listdir(fd))
        elif entry.name == ".git" and info.st_size:
            git = shutil.which("git")
            if not git:
                raise Unsafe("发现 .git 文件但无法调用 Git 确认，保留")
            paths = (
                command(
                    [
                        git,
                        "-C",
                        str(entry.parent),
                        "rev-parse",
                        "--path-format=absolute",
                        "--git-dir",
                        "--git-common-dir",
                    ]
                )
                .decode()
                .splitlines()
            )
            if len(paths) != 2:
                raise Unsafe("无法确认子树中的 .git 入口，保留")
            if os.path.normpath(paths[0]) != os.path.normpath(paths[1]):
                raise Unsafe("子树含 Git linked worktree，请用 git worktree remove 处理")
    return result


def digest(fd: int) -> bytes:
    os.lseek(fd, 0, os.SEEK_SET)
    hasher = hashlib.sha256()
    while chunk := os.read(fd, 1024 * 1024):
        hasher.update(chunk)
    return hasher.digest()


def copy_material(source: Path, expected: Fingerprint) -> tuple[Path, bytes]:
    if os.name == "nt":
        return windows_copy_material(source, expected)
    destination = MATERIALS / "Incoming" / source.relative_to(TEMP)
    with (
        directory(source.parent) as source_dir,
        directory(destination.parent, create=True) as dest_dir,
    ):
        src = os.open(source.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=source_dir)
        stage = ".partial-" + uuid.uuid4().hex
        created = False
        try:
            if fingerprint(os.fstat(src)) != expected:
                raise Unsafe(f"样本在扫描后发生变化，停止清理：{source}")
            source_hash = digest(src)
            try:
                existing = os.open(
                    destination.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=dest_dir
                )
            except FileNotFoundError:
                existing = None
            if existing is not None:
                try:
                    if (
                        not stat.S_ISREG(os.fstat(existing).st_mode)
                        or digest(existing) != source_hash
                    ):
                        raise Unsafe(f"样本目标已存在且内容不同，停止清理：{destination}")
                finally:
                    os.close(existing)
            else:
                out = os.open(
                    stage,
                    os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                    0o600,
                    dir_fd=dest_dir,
                )
                created = True
                try:
                    os.lseek(src, 0, os.SEEK_SET)
                    with (
                        os.fdopen(os.dup(src), "rb") as reader,
                        os.fdopen(os.dup(out), "wb") as writer,
                    ):
                        shutil.copyfileobj(reader, writer, 1024 * 1024)
                    os.fsync(out)
                    if digest(out) != source_hash:
                        raise Unsafe(f"样本复制校验失败，停止清理：{source}")
                finally:
                    os.close(out)
                # Link installs atomically and refuses to overwrite a concurrent destination.
                os.link(stage, destination.name, src_dir_fd=dest_dir, dst_dir_fd=dest_dir)
                os.fsync(dest_dir)
            if fingerprint(os.fstat(src)) != expected:
                raise Unsafe(f"样本在复制时发生变化，停止清理：{source}")
            return destination, source_hash
        finally:
            os.close(src)
            if created:
                os.unlink(stage, dir_fd=dest_dir)


def verify_material(path: Path, expected_hash: bytes) -> None:
    if os.name == "nt":
        return windows_verify_material(path, expected_hash)
    with directory(path.parent) as fd:
        file_fd = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
        try:
            if not stat.S_ISREG(os.fstat(file_fd).st_mode) or digest(file_fd) != expected_hash:
                raise Unsafe(f"归集样本已变化，保留源文件：{path}")
        finally:
            os.close(file_fd)


def expected_metadata(
    original: Fingerprint, unlinked: dict[Identity, tuple[Fingerprint, Fingerprint]]
) -> Fingerprint:
    previous = unlinked.get(original[:2])
    return previous[1] if previous and previous[0] == original else original


def remove_snapshot(
    path: Path, tree: Snapshot, unlinked: dict[Identity, tuple[Fingerprint, Fingerprint]]
) -> None:
    if os.name == "nt":
        return windows_remove_snapshot(path, tree)
    # Remove only scanned names. Additional names make rmdir fail rather than
    # being swept up recursively. Close writers first: these checks do not lock
    # other processes out of replacing or moving entries.
    for relative, expected in sorted(
        tree.items(), key=lambda item: len(item[0].parts), reverse=True
    ):
        entry = path / relative
        with directory(entry.parent) as fd:
            current = os.stat(entry.name, dir_fd=fd, follow_symlinks=False)
            if stat.S_ISDIR(expected[2]):
                # Removing its children changes directory size/times.
                if identity(current) != expected[:2] or not stat.S_ISDIR(current.st_mode):
                    raise Unsafe(f"目录已被替换，保留：{entry}")
                os.rmdir(entry.name, dir_fd=fd)
            else:
                wanted = expected_metadata(expected, unlinked)
                # O_PATH observes unlink's ctime change even for unreadable files.
                file_fd = os.open(entry.name, os.O_PATH | os.O_NOFOLLOW, dir_fd=fd)
                try:
                    if fingerprint(current) != wanted or fingerprint(os.fstat(file_fd)) != wanted:
                        raise Unsafe(f"文件已变化，保留：{entry}")
                    os.unlink(entry.name, dir_fd=fd)
                    if current.st_nlink > 1:
                        after = fingerprint(os.fstat(file_fd))
                        if after[:-1] != wanted[:-1]:
                            raise Unsafe(f"删除时文件内容发生变化，停止清理：{entry}")
                        unlinked[expected[:2]] = (expected, after)
                finally:
                    os.close(file_fd)


def selected_targets(values: Sequence[str]) -> list[Path]:
    """Resolve explicitly selected paths without traversing links or leaving temp."""
    result: list[Path] = []
    for value in values:
        requested = Path(value).expanduser()
        if requested.is_absolute():
            target = requested
        elif requested.parts and os.path.normcase(requested.parts[0]) == os.path.normcase("temp"):
            target = ROOT / requested
        else:
            target = TEMP / requested
        target = Path(os.path.abspath(target))
        if not within(target, TEMP) or os.path.normcase(str(target)) == os.path.normcase(str(TEMP)):
            raise Unsafe(f"清理目标必须是 temp 内的具体路径：{value}")
        if os.path.normcase(str(target)) in {
            os.path.normcase(str(TEMP / LOCK)),
            os.path.normcase(str(TEMP / "AGENTS.md")),
            os.path.normcase(str(MATERIALS)),
        }:
            raise Unsafe(f"不能选择受保护的路径：{target}")
        if os.name == "nt":
            windows_assert_directory(target.parent)
        else:
            with directory(target.parent):
                pass
        if not os.path.lexists(target):
            raise Unsafe(f"清理目标不存在：{target}")
        if not any(within(target, selected) for selected in result):
            result = [selected for selected in result if not within(selected, target)]
            result.append(target)
    return sorted(result)


def clean(args: argparse.Namespace, locked_root: Identity | None = None) -> None:
    busy = busy_paths()
    if args.apply and busy[2]:
        raise Unsafe("；".join(busy[2]))
    candidates: list[tuple[Path, Snapshot, list[tuple[Path, Fingerprint]]]] = []
    saved: dict[Path, list[tuple[Path, bytes]]] = {}
    print(f"扫描 {TEMP}（{'归集样本后清理' if args.apply else '只预览'}）...")
    targeted = bool(args.targets)
    entries = selected_targets(args.targets) if targeted else sorted(TEMP.iterdir())
    skipped = 0
    for entry in entries:
        name = entry.relative_to(TEMP).as_posix()
        if not targeted and (
            entry.name in (LOCK, "AGENTS.md", "test-materials")
            or (not args.include_toolchains and entry.name in ("tools", "venvs"))
        ):
            print(f"保留 temp/{name}")
            continue
        reason = block_reason(entry, busy)
        try:
            if reason:
                raise Unsafe(reason)
            tree = snapshot(entry)
            protected = [
                entry / relative
                for relative in tree
                if (entry / relative).name.casefold() == "agents.md"
                or (
                    within(entry / relative, MATERIALS) and (entry / relative).name.startswith("0-")
                )
            ]
            if protected:
                raise Unsafe(f"包含受保护的 Agent 指令或用户原始材料：{protected[0]}")
        except (OSError, Unsafe) as exc:
            skipped += 1
            print(f"保留 temp/{name}：{exc}")
            continue
        top = entry.relative_to(TEMP).parts[0]
        materials = [
            (entry / relative, info)
            for relative, info in tree.items()
            if stat.S_ISREG(info[2])
            and not within(entry / relative, MATERIALS)
            and is_material(entry / relative, top)
        ]
        size = sum(info[3] for info in tree.values() if stat.S_ISREG(info[2]))
        candidates.append((entry, tree, materials))
        print(f"候选 temp/{name}：{size / 1024**2:.1f} MiB，先保留 {len(materials)} 份样本")
    print(f"候选 {len(candidates)} 项。")
    if not args.apply:
        print("预览完成，未修改文件。执行清理：在本次命令中加入 --apply。")
        return
    # Finish all preservation before the first deletion. Conflicts stop the entire run.
    for entry, _, materials in candidates:
        saved[entry] = [copy_material(source, expected) for source, expected in materials]
    for copies in saved.values():
        for destination, expected_hash in copies:
            verify_material(destination, expected_hash)
    failures = skipped if targeted else 0
    unlinked: dict[Identity, tuple[Fingerprint, Fingerprint]] = {}
    for entry, tree, _ in candidates:
        try:
            with cleanup_directory(locked_root):
                reason = block_reason(entry, busy_paths())
                if reason:
                    raise Unsafe(reason)
                expected_tree = {
                    path: expected_metadata(info, unlinked) for path, info in tree.items()
                }
                if snapshot(entry) != expected_tree:
                    raise Unsafe("扫描后子树发生变化，保留")
                for destination, expected_hash in saved[entry]:
                    verify_material(destination, expected_hash)
                remove_snapshot(entry, tree, unlinked)
            print(f"已清理 temp/{entry.relative_to(TEMP).as_posix()}")
        except (OSError, Unsafe) as exc:
            failures += 1
            print(f"跳过 temp/{entry.relative_to(TEMP).as_posix()}：{exc}", file=sys.stderr)
    if not targeted:
        initialize()
    if failures:
        raise Unsafe(f"{failures} 项被保留或无法清理；请检查上述原因。")
    print("清理完成。测试材料位于 temp/test-materials/。")


@contextlib.contextmanager
def cleanup_directory(locked_root: Identity | None) -> Generator[None, None, None]:  # noqa: UP043
    if os.name == "nt":
        windows_assert_directory(TEMP)
        if identity(TEMP.stat()) != locked_root:
            raise Unsafe("temp 目录已被替换，停止清理")
        yield
    else:
        with directory(TEMP) as fd:
            if identity(os.fstat(fd)) != locked_root:
                raise Unsafe("temp 目录已被替换，停止清理")
            yield


def windows_assert_directory(path: Path, create: bool = False) -> None:
    for ancestor in reversed((path, *path.parents)):
        try:
            info = ancestor.lstat()
        except FileNotFoundError:
            if not create:
                raise
            ancestor.mkdir()
            info = ancestor.lstat()
        if info.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT or not stat.S_ISDIR(
            info.st_mode
        ):
            raise Unsafe(f"目录或其祖先不是普通目录：{ancestor}")


def windows_initialize() -> None:
    paths = [TEMP / name for name in STANDARD] + [MATERIALS / name for name in MATERIAL_DIRS]
    for path in paths:
        for ancestor in (path, *path.parents):
            try:
                info = ancestor.lstat()
            except FileNotFoundError:
                continue
            if info.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT or not stat.S_ISDIR(
                info.st_mode
            ):
                raise Unsafe(f"标准目录或其祖先不是普通目录：{ancestor}")
    for path in paths:
        windows_assert_directory(path, create=True)
    print("标准 temp 目录已就绪。")


def windows_initialize_agents() -> None:
    windows_assert_directory(TEMP)
    target = TEMP / "AGENTS.md"
    if os.path.lexists(target):
        return
    template = (Path(__file__).parent / "res/AGENTS.template.md").read_text(encoding="utf-8")
    with target.open("x", encoding="utf-8", newline="") as output:
        output.write(template.replace("<ROOT>", ".."))
    print("已从模板创建 temp/AGENTS.md。")


@contextlib.contextmanager
def windows_cleanup_lock() -> Generator[Identity, None, None]:  # noqa: UP043
    import msvcrt

    windows_assert_directory(TEMP)
    path = TEMP / LOCK
    if (
        os.path.lexists(path)
        and path.lstat().st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT
    ):
        raise Unsafe("清理锁为链接。")
    with path.open("a+b") as lock:
        lock.seek(0)
        try:
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError:
            raise Unsafe("另一个 temp 清理正在运行；本次未清理。") from None
        try:
            yield identity(TEMP.stat())
        finally:
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)


def windows_snapshot(path: Path) -> Snapshot:
    result: Snapshot = {}
    pending = [path]
    while pending:
        entry = pending.pop()
        info = entry.lstat()
        reparse = bool(info.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT)
        if reparse:
            if entry == path or entry.name.lower() == ".git":
                raise Unsafe("候选或 Git 入口为链接，保留")
            result[entry.relative_to(path)] = fingerprint(info)
            continue
        if not stat.S_ISDIR(info.st_mode) and not stat.S_ISREG(info.st_mode):
            raise Unsafe("子树含特殊文件，保留")
        result[entry.relative_to(path)] = fingerprint(info)
        if stat.S_ISDIR(info.st_mode):
            pending.extend(entry.iterdir())
        elif entry.name.lower() == ".git" and info.st_size:
            git = shutil.which("git")
            if not git:
                raise Unsafe("发现 .git 文件但无法调用 Git 确认，保留")
            paths = (
                command(
                    [
                        git,
                        "-C",
                        str(entry.parent),
                        "rev-parse",
                        "--path-format=absolute",
                        "--git-dir",
                        "--git-common-dir",
                    ]
                )
                .decode("utf-8")
                .splitlines()
            )
            if len(paths) != 2 or os.path.normcase(paths[0]) != os.path.normcase(paths[1]):
                raise Unsafe("子树含 Git linked worktree 或无法确认，请保留")
    return result


def windows_copy_material(source: Path, expected: Fingerprint) -> tuple[Path, bytes]:
    destination = MATERIALS / "Incoming" / source.relative_to(TEMP)
    windows_assert_directory(source.parent)
    windows_assert_directory(destination.parent, create=True)
    if fingerprint(source.lstat()) != expected:
        raise Unsafe(f"样本在扫描后发生变化，停止清理：{source}")
    with source.open("rb") as reader:
        source_hash = digest(reader.fileno())
    if os.path.lexists(destination):
        windows_verify_material(destination, source_hash)
    else:
        stage = destination.with_name(".partial-" + uuid.uuid4().hex)
        try:
            with source.open("rb") as reader, stage.open("xb") as writer:
                shutil.copyfileobj(reader, writer)
            windows_verify_material(stage, source_hash)
            os.link(stage, destination)
        finally:
            stage.unlink(missing_ok=True)
    if fingerprint(source.lstat()) != expected:
        raise Unsafe(f"样本在复制时发生变化，停止清理：{source}")
    return destination, source_hash


def windows_verify_material(path: Path, expected_hash: bytes) -> None:
    windows_assert_directory(path.parent)
    info = path.lstat()
    if info.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT or not stat.S_ISREG(
        info.st_mode
    ):
        raise Unsafe(f"归集样本不是普通文件：{path}")
    with path.open("rb") as reader:
        actual = digest(reader.fileno())
    if actual != expected_hash:
        raise Unsafe(f"归集样本已变化，保留源文件：{path}")


def windows_remove_snapshot(path: Path, tree: Snapshot) -> None:
    for relative, expected in sorted(
        tree.items(), key=lambda item: len(item[0].parts), reverse=True
    ):
        entry = path / relative
        windows_assert_directory(entry.parent)
        current = entry.lstat()
        if (
            stat.S_ISDIR(expected[2])
            and not current.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT
        ):
            if identity(current) != expected[:2]:
                raise Unsafe(f"目录已被替换，保留：{entry}")
            entry.rmdir()
        else:
            if fingerprint(current) != expected:
                raise Unsafe(f"文件已变化，保留：{entry}")
            if current.st_file_attributes & stat.FILE_ATTRIBUTE_DIRECTORY:
                entry.rmdir()
            else:
                entry.unlink()


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="temp",
        description="建立、查看或清理当前仓库的 temp 目录；init 从模板创建缺失的 temp/AGENTS.md。",
    )
    parser.add_argument("action", choices=("init", "status", "clean"), default="status", nargs="?")
    parser.add_argument(
        "targets", nargs="*", help="clean 指定 temp 内的文件或目录；默认扫描整个 temp"
    )
    parser.add_argument("--apply", "-Apply", action="store_true", help="实际执行 clean；默认只预览")
    parser.add_argument(
        "--include-toolchains",
        "-IncludeToolchains",
        action="store_true",
        help="clean 同时清理 tools 和 venvs",
    )
    args = parser.parse_args()
    if args.action != "clean" and (args.apply or args.include_toolchains or args.targets):
        parser.error("指定路径、--apply 和 --include-toolchains 仅适用于 clean")
    if args.action == "init":
        initialize()
        initialize_agents()
        return
    try:
        if os.name == "nt":
            windows_assert_directory(TEMP)
        else:
            with directory(TEMP):
                pass
    except FileNotFoundError:
        print("temp/ 不存在。使用 temp 入口运行 init 建立目录。")
        return
    if args.action == "status":
        for name in STANDARD:
            print(f"temp/{name}：{'已存在' if (TEMP / name).is_dir() else '未建立'}")
    if args.apply:
        # Validate preserved paths before cleanup even when there are no samples.
        for path in (MATERIALS, MATERIALS / "Incoming"):
            try:
                if os.name == "nt":
                    windows_assert_directory(path)
                else:
                    with directory(path):
                        pass
            except FileNotFoundError:
                pass
        with cleanup_lock() as locked_root:
            clean(args, locked_root)
    else:
        clean(args)


if __name__ == "__main__":
    try:
        main()
    except (OSError, Unsafe) as exc:
        print(f"错误：{exc}", file=sys.stderr)
        sys.exit(1)
