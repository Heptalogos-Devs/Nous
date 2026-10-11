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
