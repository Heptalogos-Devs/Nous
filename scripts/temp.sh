#!/bin/sh
# POSIX shell entry; the safety checks use Python 3.9+ standard library only.
# No activated virtual environment, uv, PowerShell or third-party modules needed.
set -eu
repo_root=$(CDPATH= cd -P -- "$(dirname -- "$0")/.." && pwd)
exec python3 -B - "$repo_root" "$@" <<'PY'
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import uuid
from pathlib import Path

STANDARD = ('build', 'cache', 'runtime', 'work', 'reference-code', 'tools', 'venvs', 'test-materials')
MATERIAL_DIRS = (
    'Audio/Speech/TTS', 'Audio/Speech/FormatChecks', 'Audio/Music', 'Image/Scenes',
    'Image/Text', 'Image/Illustration', 'Image/Pixel', 'Image/Screenshots',
    'Image/Photography', 'Image/Probes', 'PDF', 'Text', 'Video', 'Instances', 'Config',
    'Prompts', 'Schemas', 'Collections', 'Results/Image', 'Results/Audio/ASR',
    'Results/Audio/Music', 'Exports', 'Diagnostics/Fixtures', 'Incoming',
)
MEDIA = set(('.pdf .png .jpg .jpeg .gif .webp .bmp .tif .tiff .avif .heic .svg '
             '.wav .mp3 .flac .ogg .opus .m4a .aac .aiff .aif .wma .amr .pcm '
             '.mp4 .mov .mkv .webm .avi .m4v .mpg .mpeg '
             '.doc .docx .ppt .pptx .xls .xlsx .epub .srt .vtt').split())
EXCLUDED_TOP = re.compile(r'^(build|cache|dist|runtime|work|reference-code|wheel-installed|'
                          r'sdist-check|tools|venvs|new-api.*|native-.*|nous-.*|pytest-.*)$', re.I)
EXCLUDED_PARTS = {'.git', 'node_modules', 'cache', '__pycache__', 'target',
                  'site-packages', 'content', 'index', 'credentials'}
ROOT = Path(sys.argv[1])
TEMP = ROOT / 'temp'
MATERIALS = TEMP / 'test-materials'
LOCK = '.cleanup.lock'
DIR_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW


if sys.version_info < (3, 9):
    sys.exit('temp.sh requires Python 3.9 or newer.')


class Unsafe(Exception):
    pass


def within(path, parent):
    return os.path.commonpath((os.path.abspath(path), str(parent))) == str(parent)


@contextlib.contextmanager
def directory(path, create=False):
    """Walk from / using directory handles; never traverse a symbolic link."""
    fd = os.open('/', DIR_FLAGS)
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


def fingerprint(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size,
            info.st_mtime_ns, info.st_ctime_ns)


def identity(info):
    return info.st_dev, info.st_ino


def initialize():
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
                raise Unsafe(f'标准目录或其祖先不是普通目录：{ancestor}')
    for path in paths:
        with directory(path, create=True):
            pass
    print('标准 temp 目录已就绪。')


@contextlib.contextmanager
def cleanup_lock():
    with directory(TEMP) as root_fd:
        fd = os.open(LOCK, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600, dir_fd=root_fd)
        try:
            if not stat.S_ISREG(os.fstat(fd).st_mode):
                raise Unsafe('清理锁不是普通文件。')
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise Unsafe('另一个 temp 清理正在运行；本次未清理。') from None
            yield identity(os.fstat(root_fd))
        finally:
            os.close(fd)


def is_material(path, top):
    if EXCLUDED_TOP.fullmatch(top):
        return False
    if any(part.lower() in EXCLUDED_PARTS for part in path.relative_to(TEMP).parts):
        return False
    ext = path.suffix.lower()
    if ext in MEDIA:
        return True
    if top.lower() in ('media-verification', 'music-samples', 'volcengine-tts', 'volcengine-tts-check'):
        return ext in ('.json', '.txt', '.md', '.html', '.ffconcat', '.ssml', '.bin')
    return (ext == '.md' or path.name.lower() == 'custom-array-schema.json'
            or (ext == '.txt' and re.search(r'material|sample|transcript|script', path.stem, re.I)))


def command(args):
    try:
        result = subprocess.run(args, capture_output=True, check=True, timeout=20)
        return result.stdout
    except (OSError, subprocess.SubprocessError):
        raise Unsafe(f'无法完成 {args[0]} 检查，保留候选。') from None


def snapshot(path):
    """Do not follow links; reject mount points and linked worktrees; capture changes."""
    result = {}
    pending = [path]
    while pending:
        entry = pending.pop()
        info = entry.lstat()
        if stat.S_ISLNK(info.st_mode):
            if entry == path or entry.name == '.git':
                raise Unsafe('候选或 Git 入口为符号链接，保留')
            result[entry.relative_to(path)] = fingerprint(info)
            continue
        if not stat.S_ISDIR(info.st_mode) and not stat.S_ISREG(info.st_mode):
            raise Unsafe('子树含特殊文件，保留')
        if os.path.ismount(entry):
            raise Unsafe('子树含挂载点，保留')
        result[entry.relative_to(path)] = fingerprint(info)
        if stat.S_ISDIR(info.st_mode):
            with directory(entry) as fd:
                pending.extend(entry / name for name in os.listdir(fd))
        elif entry.name == '.git':
            git = shutil.which('git')
            if not git:
                raise Unsafe('发现 .git 文件但无法调用 Git 确认，保留')
            paths = command([git, '-C', str(entry.parent), 'rev-parse', '--path-format=absolute',
                             '--git-dir', '--git-common-dir']).decode().splitlines()
            if len(paths) != 2:
                raise Unsafe('无法确认子树中的 .git 入口，保留')
            if os.path.normpath(paths[0]) != os.path.normpath(paths[1]):
                raise Unsafe('子树含 Git linked worktree，请用 git worktree remove 处理')
    return result


def process_references():
    if not sys.platform.startswith('linux') or not Path('/proc/self/status').exists():
        return [], ['当前平台无法检查进程的命令行、工作目录及打开文件']
    processes, errors = [], []
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit():
            continue
        try:
            status = (proc / 'status').read_text()
            if re.search(r'^State:\s+[ZX]\b|^Kthread:\s+1\b', status, re.M):
                continue
            uid = re.search(r'^Uid:\s+(\d+)', status, re.M)
            owned = uid is not None and int(uid[1]) == os.getuid()
            name = re.search(r'^Name:\s+(.+)', status, re.M)[1]
        except (OSError, TypeError):
            try:
                if proc.stat().st_uid == os.getuid():
                    errors.append(f'无法检查当前用户进程 {proc.name}')
            except FileNotFoundError:
                pass
            continue
        paths, unreadable = [], []
        arguments, cwd = [], None
        try:
            arguments = [os.fsdecode(arg) for arg in (proc / 'cmdline').read_bytes().split(b'\0') if arg]
        except OSError:
            unreadable.append('cmdline')
        for field in ('exe', 'cwd'):
            try:
                raw_target = os.readlink(proc / field)
                target = raw_target.removesuffix(' (deleted)')
                paths.extend((raw_target, target))
                if field == 'cwd':
                    cwd = target
            except OSError:
                unreadable.append(field)
        try:
            for descriptor in (proc / 'fd').iterdir():
                try:
                    target = os.readlink(descriptor)
                    if target.startswith('/'):
                        paths.extend((target, target.removesuffix(' (deleted)')))
                except FileNotFoundError:
                    pass  # Descriptors can close while scanning.
                except OSError:
                    unreadable.append('fd')
        except OSError:
            unreadable.append('fd')
        try:
            current = (proc / 'status').read_text()
            if re.search(r'^State:\s+[ZX]\b', current, re.M):
                continue
        except FileNotFoundError:
            continue
        except OSError:
            unreadable.append('status')
        if owned and unreadable:
            errors.append(f'无法完整检查当前用户进程 {proc.name} ({name})：{", ".join(set(unreadable))}')
        if cwd:
            for arg in arguments:
                value = arg.split('=', 1)[1] if arg.startswith('-') and '=' in arg else arg
                value = value.removeprefix('@')
                if value and not value.startswith('-'):
                    lexical = os.path.abspath(os.path.join(cwd, value))
                    paths.extend((lexical, os.path.realpath(lexical)))
        processes.append((proc.name, name, paths, arguments))
    return processes, errors


def busy_paths():
    processes, errors = process_references()
    mounts = []
    if sys.platform.startswith('linux'):
        try:
            for line in Path('/proc/self/mountinfo').read_text().splitlines():
                # mountinfo uses octal escapes, including spaces and backslashes.
                field = line.split()[4]
                mounts.append((re.sub(r'\\([0-7]{3})', lambda m: chr(int(m[1], 8)), field), False))
        except (OSError, IndexError):
            errors.append('无法检查当前挂载点')
    docker = shutil.which('docker')
    if docker:
        try:
            ids = command([docker, 'ps', '--quiet']).decode().split()
            if ids:
                for container in json.loads(command([docker, 'inspect', *ids])):
                    mounts.extend((mount['Source'], True) for mount in container['Mounts'] if mount['Type'] == 'bind')
        except (Unsafe, ValueError, KeyError, TypeError):
            errors.append('无法检查运行中的 Docker bind mount')
    return processes, mounts, errors


def block_reason(path, busy):
    processes, mounts, errors = busy
    if within(os.getcwd(), path):
        return '当前工作目录位于其中'
    # Ordinary filesystem ancestors (/, /tmp) do not make every path busy.
    if any(within(mount, path) or (bind and within(path, mount)) for mount, bind in mounts):
        return '子树含挂载点或运行中的 Docker bind mount'
    for pid, name, paths, args in processes:
        if (any(within(target, path) for target in paths)
                or any(str(path) in arg for arg in args)
                or (path.name in ('build', 'cache') and name in ('uv', 'cargo', 'rustc', 'go', 'bun'))):
            return f'进程 {pid} ({name}) 正在使用'
    return '；'.join(errors) if errors else None


def digest(fd):
    os.lseek(fd, 0, os.SEEK_SET)
    hasher = hashlib.sha256()
    while chunk := os.read(fd, 1024 * 1024):
        hasher.update(chunk)
    return hasher.digest()


def copy_material(source, expected):
    destination = MATERIALS / 'Incoming' / source.relative_to(TEMP)
    with directory(source.parent) as source_dir, directory(destination.parent, create=True) as dest_dir:
        src = os.open(source.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=source_dir)
        stage = '.partial-' + uuid.uuid4().hex
        created = False
        try:
            if fingerprint(os.fstat(src)) != expected:
                raise Unsafe(f'样本在扫描后发生变化，停止清理：{source}')
            source_hash = digest(src)
            try:
                existing = os.open(destination.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=dest_dir)
            except FileNotFoundError:
                existing = None
            if existing is not None:
                try:
                    if not stat.S_ISREG(os.fstat(existing).st_mode) or digest(existing) != source_hash:
                        raise Unsafe(f'样本目标已存在且内容不同，停止清理：{destination}')
                finally:
                    os.close(existing)
            else:
                out = os.open(stage, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=dest_dir)
                created = True
                try:
                    os.lseek(src, 0, os.SEEK_SET)
                    with os.fdopen(os.dup(src), 'rb') as reader, os.fdopen(os.dup(out), 'wb') as writer:
                        shutil.copyfileobj(reader, writer, 1024 * 1024)
                    os.fsync(out)
                    if digest(out) != source_hash:
                        raise Unsafe(f'样本复制校验失败，停止清理：{source}')
                finally:
                    os.close(out)
                # Link installs atomically and refuses to overwrite a concurrent destination.
                os.link(stage, destination.name, src_dir_fd=dest_dir, dst_dir_fd=dest_dir)
                os.fsync(dest_dir)
            if fingerprint(os.fstat(src)) != expected:
                raise Unsafe(f'样本在复制时发生变化，停止清理：{source}')
            return destination, source_hash
        finally:
            os.close(src)
            if created:
                os.unlink(stage, dir_fd=dest_dir)


def verify_material(path, expected_hash):
    with directory(path.parent) as fd:
        file_fd = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
        try:
            if not stat.S_ISREG(os.fstat(file_fd).st_mode) or digest(file_fd) != expected_hash:
                raise Unsafe(f'归集样本已变化，保留源文件：{path}')
        finally:
            os.close(file_fd)


def expected_metadata(original, unlinked):
    previous = unlinked.get(original[:2])
    return previous[1] if previous and previous[0] == original else original


def remove_snapshot(path, tree, unlinked):
    # Remove only scanned names. Additional names make rmdir fail rather than
    # being swept up recursively. Close writers first: these checks do not lock
    # other processes out of replacing or moving entries.
    for relative, expected in sorted(tree.items(), key=lambda item: len(item[0].parts), reverse=True):
        entry = path / relative
        with directory(entry.parent) as fd:
            current = os.stat(entry.name, dir_fd=fd, follow_symlinks=False)
            if stat.S_ISDIR(expected[2]):
                # Removing its children changes directory size/times.
                if identity(current) != expected[:2] or not stat.S_ISDIR(current.st_mode):
                    raise Unsafe(f'目录已被替换，保留：{entry}')
                os.rmdir(entry.name, dir_fd=fd)
            else:
                wanted = expected_metadata(expected, unlinked)
                # O_PATH observes unlink's ctime change even for unreadable files.
                file_fd = os.open(entry.name, os.O_PATH | os.O_NOFOLLOW, dir_fd=fd)
                try:
                    if fingerprint(current) != wanted or fingerprint(os.fstat(file_fd)) != wanted:
                        raise Unsafe(f'文件已变化，保留：{entry}')
                    os.unlink(entry.name, dir_fd=fd)
                    after = fingerprint(os.fstat(file_fd))
                    if after[:-1] != wanted[:-1]:
                        raise Unsafe(f'删除时文件内容发生变化，停止清理：{entry}')
                    unlinked[expected[:2]] = (expected, after)
                finally:
                    os.close(file_fd)


def clean(args, locked_root=None):
    busy = busy_paths()
    if args.apply and busy[2]:
        raise Unsafe('；'.join(busy[2]))
    candidates, saved = [], {}
    print(f'扫描 {TEMP}（{"归集样本后清理" if args.apply else "只预览"}）...')
    for entry in sorted(TEMP.iterdir()):
        if entry.name in (LOCK, 'test-materials') or (not args.include_toolchains and entry.name in ('tools', 'venvs')):
            print(f'保留 temp/{entry.name}')
            continue
        reason = block_reason(entry, busy)
        try:
            if reason:
                raise Unsafe(reason)
            tree = snapshot(entry)
        except (OSError, Unsafe) as exc:
            print(f'保留 temp/{entry.name}：{exc}')
            continue
        materials = [(entry / relative, info) for relative, info in tree.items()
                     if stat.S_ISREG(info[2]) and is_material(entry / relative, entry.name)]
        size = sum(info[3] for info in tree.values() if stat.S_ISREG(info[2]))
        candidates.append((entry, tree, materials))
        print(f'候选 temp/{entry.name}：{size / 1024**2:.1f} MiB，先保留 {len(materials)} 份样本')
    print(f'候选 {len(candidates)} 项。')
    if not args.apply:
        print('预览完成，未修改文件。执行清理：sh scripts/temp.sh clean --apply')
        return
    # Finish all preservation before the first deletion. Conflicts stop the entire run.
    for entry, _, materials in candidates:
        saved[entry] = [copy_material(source, expected) for source, expected in materials]
    for copies in saved.values():
        for destination, expected_hash in copies:
            verify_material(destination, expected_hash)
    failures = 0
    unlinked = {}
    for entry, tree, _ in candidates:
        try:
            with directory(TEMP) as fd:
                if identity(os.fstat(fd)) != locked_root:
                    raise Unsafe('temp 目录已被替换，停止清理')
                reason = block_reason(entry, busy_paths())
                if reason:
                    raise Unsafe(reason)
                expected_tree = {path: expected_metadata(info, unlinked) for path, info in tree.items()}
                if snapshot(entry) != expected_tree:
                    raise Unsafe('扫描后子树发生变化，保留')
                for destination, expected_hash in saved[entry]:
                    verify_material(destination, expected_hash)
                remove_snapshot(entry, tree, unlinked)
            print(f'已清理 temp/{entry.name}')
        except (OSError, Unsafe) as exc:
            failures += 1
            print(f'跳过 temp/{entry.name}：{exc}', file=sys.stderr)
    initialize()
    if failures:
        raise Unsafe(f'{failures} 项在清理前变化或无法清理；请关闭相关任务后重试。')
    print('清理完成。测试材料位于 temp/test-materials/。')


def main():
    parser = argparse.ArgumentParser(prog='temp.sh', description='建立、查看或清理当前仓库的 temp 目录。Python 3.9+；Linux 支持清理检查。')
    parser.add_argument('action', choices=('init', 'status', 'clean'), default='status', nargs='?')
    parser.add_argument('--apply', '-Apply', action='store_true', help='实际执行 clean；默认只预览')
    parser.add_argument('--include-toolchains', '-IncludeToolchains', action='store_true', help='clean 同时清理 tools 和 venvs')
    args = parser.parse_args(sys.argv[2:])
    if args.action != 'clean' and (args.apply or args.include_toolchains):
        parser.error('--apply 和 --include-toolchains 仅适用于 clean')
    if args.action == 'init':
        initialize()
        return
    try:
        with directory(TEMP):
            pass
    except FileNotFoundError:
        print('temp/ 不存在。建立目录：sh scripts/temp.sh init')
        return
    if args.action == 'status':
        for name in STANDARD:
            print(f'temp/{name}：{"已存在" if (TEMP / name).is_dir() else "未建立"}')
    if args.apply:
        # Validate preserved paths before cleanup even when there are no samples.
        for path in (MATERIALS, MATERIALS / 'Incoming'):
            try:
                with directory(path):
                    pass
            except FileNotFoundError:
                pass
        with cleanup_lock() as locked_root:
            clean(args, locked_root)
    else:
        clean(args)


try:
    main()
except (OSError, Unsafe) as exc:
    print(f'错误：{exc}', file=sys.stderr)
    sys.exit(1)
PY
