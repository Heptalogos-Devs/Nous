# 第三方 Runtime 构建与打包

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

这里只生产第三方运行包。普通启动消费已经安装的 pack；下载和编译是显式准备动作。

| 文件 | 用途 |
| --- | --- |
| [pack.ts](pack.ts) | runtime inventory、ZIP、manifest 和 catalog |
| [build-postgresql.sh](build-postgresql.sh) | 固定来源的 Windows PostgreSQL 构建 |
| [build-ffmpeg.sh](build-ffmpeg.sh) | 固定来源的 FFmpeg/ffprobe 构建 |
| [build-notices.sh](build-notices.sh) | 构建脚本共用的实际工具链 notices 准备 |
| [postgresql-llvm-setjmp.patch](postgresql-llvm-setjmp.patch) | LLVM-MinGW setjmp 兼容修正 |
| [postgresql-background-processes.patch](postgresql-background-processes.patch) | PostgreSQL Windows 后台进程行为修正 |

## Runtime 构建与打包

这些脚本生成第三方 runtime，不生成用户配置。Windows x64 的 shipping runtime 使用固定 source digest 和 LLVM-MinGW UCRT。

```bash
bash scripts/runtime/build-postgresql.sh <source.tar> <llvm-mingw-dir> <data/runtime/build 下的输出目录> <data/cache/runtime-build 下的构建目录>
bash scripts/runtime/build-ffmpeg.sh <source.tar> <llvm-mingw.tar> <data/runtime/build 下的输出目录> <data/cache/runtime-build 下的构建目录>
```

PostgreSQL 输出包括 postgres、initdb、pg_ctl、pg_isready 和所需库；FFmpeg 输出包括 ffmpeg、ffprobe。两个构建脚本调用 `build-notices.sh` 收集工具链许可证，需网络；PostgreSQL 使用同目录的 setjmp patch。build-root 是可复用构建目录。

```text
corepack pnpm runtime:pack --component postgresql --version 18.6.0 --source <来源URL> --license <许可证名> --root <runtime目录> --output data/runtime
```

component/version/source/license/root 五个参数必填；component 为 `node`、`postgresql` 或 `ffmpeg`。runtime 目录必须已有必要可执行文件和 license notices。命令写入文件 digest manifest、`data/runtime/packs/` ZIP 和 `data/runtime/manifest/runtimes.json` catalog；当前只支持 Windows x64。`--output` 是 pack/catalog 输出根，默认 `data/runtime/`；runtime 目录和输出根均位于仓库 `data/` 下。

## Linux 本机 FFmpeg

```bash
bash scripts/runtime/build-ffmpeg.sh --linux data/cache/runtime-sources/ffmpeg-9.0.2.tar.xz data/runtime/installed/ffmpeg
```

同一固定官方 source SHA 验证后使用现有 gcc/make 构建，输出 `bin/ffmpeg`、`bin/ffprobe` 与 `licenses/ffmpeg` 中的 source archive、许可证、build flags、工具链和可执行文件摘要。GIF parser/decoder、PNG decoder 与有界 sampler 所需 filter 在同一组件内；Core 和真实抽帧 smoke 共用该路径。Windows 原交叉构建入口保留。
