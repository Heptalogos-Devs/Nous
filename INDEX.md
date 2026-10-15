# Nous Wave 仓库地图

本页只回答“哪个边界负责什么、从哪里继续阅读”。具体行为和命令由各入口文档或机器可读合同负责。

| 区域 | 入口 | 责任边界 |
| --- | --- | --- |
| 人类文档 | [docs/INDEX.md](docs/INDEX.md) | 当前架构、状态、产品合同与研究 |
| Applications | [apps/README.md](apps/README.md) | Public Core host 与 private Kernel process composition |
| 长期目标设计 | [Architecture-Vault TARGET_DESIGN.md](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/docs/Nous-Wave/TARGET_DESIGN.md) | 长期语义、决定、理由和研究 |
| TypeScript Core | [apps/nous-core/README.md](apps/nous-core/README.md) | 公共 Core、WorkContext/Projection/Context、NousQL 和模型编排 |
| Reference consumer | [apps/nous-cli/README.md](apps/nous-cli/README.md) | official Client CLI、流式上传、Subject/Session、NousQL 和来源追踪 |
| Rust Kernel | [apps/nous-kernel/README.md](apps/nous-kernel/README.md) | 私有 Kernel 进程和 Rust owners 的组合 |
| Rust owners | [crates/README.md](crates/README.md) / [crates/INDEX.md](crates/INDEX.md) | Subject、Runtime、Material、Memory、Self、Persistence 和 Retrieval |
| Protobuf source | [proto/README.md](proto/README.md) | 唯一的跨语言 wire-contract source |
| TypeScript packages | [packages/README.md](packages/README.md) | 官方 Client 和生成的 TypeScript protocol bindings |
| 命令入口 | [justfile](justfile) / [package.json](package.json) | Rust、Protobuf、TypeScript 和 按需 public smoke |
| 脚本与维护 | [scripts/INDEX.md](scripts/INDEX.md) | 开发、运行时、发布、研究与维护脚本 |
| 工具配置 | [.config/README.md](.config/README.md) | 显式工具设置与自动发现文件 |

`crates/`、`apps/`、`packages/`、`proto/` 和 `scripts/` 的局部 `README.md` 负责解释各自边界；局部 `AGENTS.md` 只补充该区域相对根规则的 AI 操作约束。

## 包与研究素材

- [Nous Wave TypeScript Client](packages/client/README.md)
- [TypeScript Protocol Bindings](packages/protocol-ts/README.md)
- [正式 OpenClaw 认知插件](packages/openclaw/README.md)
- [研究语料](docs/research/corpus/README.md)
