# 安装 Python 服务与 native 扩展

主应用、客户端与原生计算分别是 `nous-wave-server`、`nous-wave`、`nous-wave-native`。服务使用标准 CPython 3.14，包含 GIL；独立 SDK 仍支持 Python 3.11 及以上。用户安装匹配平台的 native wheel，无需 Rust、C++、Node 或旧 Rust 服务。

## 安装发行包

从同一次构建取得对应平台的完整 `nous-wave-cp314-<target>` 产物，解压后应包含三个同版本 wheel 和 `install-python.py`。Linux x64、Windows x64、macOS Apple Silicon、macOS Intel 分别使用自己的产物。不要混合不同发行版本。

已安装 uv 时，在解压目录运行：

```bash
uv run --python 3.14 --no-project install-python.py --wheels .
```

已有标准 CPython 3.14 时也可运行 `python install-python.py --wheels .`；脚本优先使用 uv，否则使用标准 `venv` 与 pip。三个项目 wheel 在一次依赖解析中安装，强制使用二进制分发；平台不匹配时安装失败，不会偷偷编译 Rust。普通第三方依赖从包索引下载，完整离线 wheelhouse 可加 `--offline`。

安装目录和实例目录独立，可显式指定：

```bash
python install-python.py --wheels . --venv /path/to/venv --instance /path/to/instance
```

默认使用当前用户的数据目录：Linux 为 `~/.local/share/nous-wave`，macOS 为 `~/Library/Application Support/NousWave`，Windows 为 `%LOCALAPPDATA%\NousWave`。安装器保留已有实例和管理凭据，创建新实例时只输出凭据文件路径。凭据正文留在 `<instance>/credentials/admin.token`，不要写入命令日志或提交仓库。

安装器会输出适用于当前系统的完整启动命令：

```bash
/path/to/venv/bin/nous serve --instance /path/to/instance --host 127.0.0.1 --port 7441
```

Windows 对应 `C:\path\to\venv\Scripts\nous.exe`。通过同一安装的 `nous --help` 查看 CLI；HTTP 管理界面与 API 使用该实例的凭据。实例数据库、内容、索引和后台任务均随实例保存，程序升级不需要移动它们。

音视频探测、裁剪与采样另需 PATH 中的 `ffmpeg` 和 `ffprobe`。Debian/Ubuntu 可安装 `ffmpeg`，macOS 可使用 Homebrew 的 `ffmpeg`，Windows 可安装 FFmpeg 并将其 `bin` 目录加入 PATH。图片由 Pillow 处理，PDF 由 pypdf 处理。缺少音视频工具会返回明确的依赖不可用错误。

## 使用 CLI

在线命令通过同一 SDK 调用服务。全局连接、语言和输出选项放在命令前；默认输出可读摘要，`--json` 的标准输出是一个 JSON 对象，失败时含 `error` 且退出码非零。列表使用 `items` 与 `count`。

```bash
nous --instance /path/to/instance status
nous --instance /path/to/instance --locale zh-CN namespace list
nous --instance /path/to/instance --json subject list NAMESPACE_ID
nous --instance /path/to/instance import NAMESPACE_ID SUBJECT_ID notes.txt --no-llm --vectors disabled
nous --instance /path/to/instance query NAMESPACE_ID SUBJECT_ID rollback --mode lexical
```

远程服务加 `--url https://host.example` 和 `--credential-file /path/to/key`。也可设置 `NOUS_INSTANCE`、`NOUS_URL`、`NOUS_CREDENTIAL_FILE`、`NOUS_LOCALE`。凭据从文件读取，不作为命令行参数。`--source-scope scopes.json` 接受 SDK `SourceReadScope` 对象数组，按请求与凭据已有权限取交集。

`source` 提供目录、读取、预览、下载、移入回收站、恢复和清除；修改来源时使用 `--reason`，预览得到的摘要可传 `--preview`。`catalog`、`cognition`、`media`、`bundle`、`context` 的结构化命令接受对应 SDK 公共合同的 JSON 文件；文件路径 `-` 表示从标准输入读取。各组 `--help` 列出实际参数。上下文准入和反馈应填写宿主实际测量的数据。

`jobs` 查询、等待、取消与恢复持久任务；`config`、`prompts` 管理配置与提示词。`credentials create command.json --token-out /path/to/new-key` 以独占方式创建凭据文件，已有文件会拒绝覆盖；省略 `--token-out` 时只在创建响应显示一次凭据。作用域内的管理凭据可管理被授予范围内的密钥与配置，完整实例备份、全局提示词和模型资源修改需要完整实例管理权限。

```bash
nous --instance /path/to/instance backup --output nous-backup.zip
nous restore nous-backup.zip /path/to/new-instance
```

恢复只接受新的目标实例；恢复后的管理凭据由新实例初始化生成。

## 当前用户后台服务

```bash
nous --instance /path/to/instance service install --autostart
nous --instance /path/to/instance service start
nous --instance /path/to/instance service status
nous --instance /path/to/instance service stop
nous --instance /path/to/instance service uninstall
```

Linux 使用 `systemd --user`，macOS 使用当前用户 LaunchAgent，Windows 使用交互用户 Task Scheduler；安装不会立即启动服务，`start` 显式启动。服务使用当前虚拟环境的 `nous`，替换安装路径后应先卸载旧定义再安装。卸载保留实例数据。容器或缺少用户会话的机器会返回服务不可用，此时直接运行 `nous serve --instance ...`。

## 从源码开发或构建

Linux 云端准备脚本为 `bash scripts/cloud-prepare.sh`。它复用本地已有 AGENTS/skills，不下载或安装治理仓库，不执行模型请求、不启动服务。系统需预先具备 C/C++ 编译器、pkg-config、CMake、FFmpeg；脚本安装/复用 uv、CPython 3.14、锁定的 Rust 工具链，再执行 `uv sync --locked --all-packages`。

```bash
uv build --package nous-wave-server --sdist --wheel --no-sources --out-dir dist
uv build --package nous-wave --sdist --wheel --no-sources --out-dir dist
uv build --package nous-wave-native --sdist --no-sources --out-dir dist
uv run maturin build --locked --release --manifest-path native/python/Cargo.toml --interpreter python3.14 --out dist
python scripts/install-python.py --wheels dist --venv /tmp/nous-installed --instance /tmp/nous-instance
```

构建始终使用 `native/python/Cargo.toml` 和 `native/Cargo.lock`。不要执行旧根目录 Cargo workspace 的应用构建。Windows 源码构建需要 Visual Studio C++ Build Tools、Windows SDK 与 CMake；macOS 需要对应架构的 Xcode Command Line Tools。Python 依赖使用根 `uv.lock`，第三方源码和构建缓存留在 `temp/`。

`.github/workflows/python-native-release.yml` 只由手动 `workflow_dispatch` 启动。它构建三个包的 sdist、server/SDK 通用 wheel，以及四个平台的 native wheel；每个平台再安装完整三件套、初始化实例并实际启动 HTTP 服务。产物保存在 Actions artifacts，不发布包索引。

Linux 发布构建使用 manylinux 2.28 容器。当前本机 Ubuntu 构建得到的 `manylinux_2_39_x86_64` wheel 需要 glibc 2.39 或更新版本，不能当作支持旧 Linux 的通用包。Windows/macOS runner 配置是构建路径，运行结果以对应 Actions 任务为准。

native 分发保留已有 `nous_native/licenses/` 中的 Apache 文本及 VCP 的 `CC-BY-NC-SA-4.0` 文本，其限制适用于对应采用代码。server/SDK 尚未声明整体许可证。
