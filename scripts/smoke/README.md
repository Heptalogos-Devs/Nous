# 公共接口确定性场景

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

`corepack pnpm smoke:longitudinal` 通过 Rust harness 启动隔离 PostgreSQL 和真实 Kernel gRPC，托管真实 Core HTTP，并由官方 Client 执行 Observation → Episode → Journal → Memory/Schema、维护机会、分页、故障后的成果复用与重启恢复。模型 proposal 使用确定性替身。该场景也由 `cargo test -p nous-kernel --test longitudinal_smoke` 执行；选择一个入口即可。

需要已准备的 PostgreSQL runtime。`NOUS_WAVE_POSTGRES_RUNTIME` 可指定安装位置。`just smoke` 与 `corepack pnpm smoke` 使用同一场景；workspace Rust tests 已执行它时无需重复运行。

其余验证按实际 owner 执行：Kernel integration tests 检查 Authority、来源权限、历史、时间域、并发与恢复；Core 的 model boundary tests 检查协议和媒体通道；`pnpm openclaw:custody:verify` 检查真实原生 SQLite、delivery、compaction、工具授权与子任务交接。官方 Client 的仓库外安装和 generated 类型由 `pnpm client:verify` 检查。

`NOUS_MEDIA_FFMPEG=/absolute/path/ffmpeg pnpm exec tsx scripts/smoke/animated-media.ts` 使用 Pillow 生成四色 GIF/PNG，并调用正式 sampler 与原生 FFmpeg，检查颜色变化、时间戳、有界截断和 PNG decoder。它不调用模型。

实际模型质量、QQ 收发和主体生活沿已授权的产品任务验证，复用原素材与已经保存的结果。
