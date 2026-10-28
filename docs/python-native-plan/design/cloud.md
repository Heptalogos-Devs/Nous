# 云端 Codex 环境与夜间工作

## 环境和源分支

在Codex选择云端工作环境，连接Nous-Wave仓库并以暂停分支为起点。新的环境需要准备、保存并发布，新的任务才使用这份准备结果。当前官方界面支持Install script与Start skill；旧界面如仍显示setup/maintenance字段，将准备脚本和启动说明放入其对应入口，保留同一工程行为。

本方案未替用户创建环境、提交仓库或启动云端任务。实际工作须在用户选定的新分支进行；云端连接的GitHub权限仅需读取源分支并将工作推到新分支/PR，禁止自动合并。

## 可直接交给环境准备代理的要求

采用Linux开发环境，安装CPython3.14标准构建、uv、Git、C/C++编译工具、pkg-config、Rust原项目所用稳定工具链、rustfmt/clippy，以及媒体所需ffmpeg/ffprobe。Python依赖从项目uv.lock安装。native只构建 `native/python/Cargo.toml`，不构建旧Rust应用工作区。

安装Basedpyright和Ruff的Language Server与CLI；VSCode类宿主读取包内编辑器配置，纯命令行宿主运行同一检查器。缓存、第三方源码、编译和运行实例统一放在temp各子目录；保留有用的uv/Cargo缓存供下次环境使用。

将 [cloud-prepare.sh](../scripts/cloud-prepare.sh) 放入仓库的scripts后，在环境安装阶段执行。脚本能在Python工程尚未建立时先准备工具，存在主应用pyproject后再同步依赖。完成首次锁定后提交uv.lock，后续使用locked模式。

云端环境更新仓库文件时，不假定安装脚本自动重跑。新增依赖或native签名变更后，当前任务主动同步对应包；如需让未来任务继承，更新并发布环境。不要每次启动无条件运行全部测试和模型演示。

## 模型连接是真实的外部前提

交接中的 New API 网关在开发电脑 `127.0.0.1:3000`，凭据和实例位于本地temp。电脑关机后该服务不可达，GitHub提交也没有携带密钥和数据。

在启动夜间完整dogfooding前，为云端提供一个实际可访问的HTTPS模型入口及该入口凭据，或直接提供各Provider的云端凭据。通过Codex环境的secret/凭据管理配置，代码只引用环境SecretRef；不要把笔记本admin token、模型key或整个temp上传到Git。

当前Codex官方云环境说明支持HTTP/network secret：进程得到替代值，经HTTPS代理对指定域名的443端口请求替换为真实值。SDK需要保持云端代理与CA配置，不可把替代值按真实key格式提前拒绝。旧版环境的setup-only secret有不同作用时机，应按所用界面选择能供实际任务调用的凭据配置。

建议用项目配置引用以下环境项，名称可沿实际Provider配置合并：

```text
NOUS_MODEL_ENDPOINT=https://实际云端网关域名/v1/
NOUS_MODEL_API_KEY=<由云端secret提供>
NOUS_MODEL_NAME=<实际已授权生成模型>
NOUS_EMBEDDING_MODEL=<实际文字或图文向量模型>
NOUS_RERANK_MODEL=<实际重排模型，若启用>
```

不同Provider有不同key时分别注册SecretRef。模型可用能力与具体路径从其真实API确定，不把所有模型硬塞入chat/completions。现有Ark视觉向量可能走特有端点，需相应域名与账户授权。配置新地址后用真实小规模操作完成检查，费用受已授权预算控制。

## 网络与依赖

准备时允许PyPI及文件托管、Python运行时下载、Rust crates/toolchain、GitHub源仓库、所选模型及必要媒体来源。受限网络使用包管理器默认允许域和确实需要的额外域；如果环境提供统一代理，HTTPX与SDK应沿用。执行者读取实际连接失败信息追加准确缺失域，不一次性为调试开放无关网络。

云端可以运行SQLite、DBOS、索引及MCP，无需新增PostgreSQL、Redis或Docker作为基础前提。公开真实资料可直接下载；私人材料仅使用本次授权且已经提供的文件。原机离线不影响云端继续，但它也无法把原机磁盘文件即时变成可读输入。

## AntiRLHF 安装与使用

先查看环境已有 `~/.agents/skills` 或 `$CODEX_HOME/skills`。若已存在新技能与宪章，则从本地读取。缺失时按用户授权下载一次：

```bash
git clone --depth 1 https://github.com/AntiRLHF-Research/AntiRLHF.git temp/upstream/AntiRLHF
python3 temp/upstream/AntiRLHF/tools/install.py \
  --target "$HOME/.agents/skills" \
  --instruction "${CODEX_HOME:-$HOME/.codex}/AGENTS.md"
```

安装器保留其他技能与非托管指令，只更新本包技能和宪章托管段。实际已有skills目录按官方installer规则沿用，脚本已处理常见目录选择。不要覆盖仓库AGENTS中的用户指令。

新仓库当前宪章讨论位于 `docs/charter/Revised.md`，常驻规则为 `CONSTITUTION.zh-CN.md`。规划时读取system-architecture、dependency-selection；迁移时读取structural-refactoring，事务与异步问题分别读persistence-transactions、async-lifecycle。其他专项知识遇到实际责任再读。不要为每个任务重复下载、拉版本或加载全部技能。

任务执行要求已写入 [GOAL](../GOAL.md)：真正的用户功能、成熟实现复用、修正所有者、整体消费者迁移、少量重要验证、完成后结束当前问题。环境准备不运行额外的证明性审计或测试平台。

## 启动任务

环境准备完并发布后，在新任务中提供 [GOAL](../GOAL.md) 和本包路径，确认目标分支是新分支。任务先读暂停交接，保留原源码；沿设计的完整能力顺序重写。云端可在电脑关闭时执行，任务工作与提交均保存在新分支。

正式工作进展保存在新分支提交，长任务中断保留当前实际状态。原分支、原机实例和真实付费结果保持原样。最终交付包括可安装Python产品、native wheel/构建源、完整公共功能、实际使用结果，以及确切尚未运行的平台或外部阻塞。
