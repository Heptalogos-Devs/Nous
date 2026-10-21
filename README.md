# Nous Wave

通用长期认知服务，可独立用于记忆应用，也可与Heptalogos联合运行。重建保留已有认知、NousQL与VCP/Native算法成果，先完成公共工程底座，再接回完整领域能力。

当前方案位于工作区`../Specs/`，发布入口为[工程设计](https://github.com/Heptalogos-Devs/Architecture-Vault/tree/feat/specs-infrastructure-research)。实际进度与未完能力见[当前施工](docs/construction.md)，产品说明见[文档目录](docs/INDEX.md)。

## 代码结构

- `packages/contracts`：纯Zod合同、共同值和HTTP元数据/Problem处理。
- `packages/platform`：普通共享库，包括目录、引导、凭据、配置、存储、网络、模型、Prompt、时间和观测；各产品独立持有实例与状态。
- `packages/nous`：认知模块、本产品时间引用、原生调用事实接线和自己的迁移。
- `apps/server`、`apps/cli`、`packages/client`：正式HTTP、离线/在线管理和生成SDK。
- `integrations/openclaw`：原生逐调用认知与计费接缝、正式源码补丁；工具、历史和执行仍由OpenClaw拥有。

## 本地启动

使用Node24、TypeScript7、pnpm12.8.1。以下命令在Ubuntu工作区执行，运行数据和开发缓存进入相邻`temp/`：

```bash
export XDG_CACHE_HOME="$PWD/../temp/cache"
export COREPACK_HOME="$PWD/../temp/corepack"
export npm_config_cache="$PWD/../temp/cache/npm"
corepack pnpm install --store-dir ../temp/pnpm-store --state-dir ../temp/pnpm-state
corepack pnpm build
corepack pnpm nous --instance ../temp/nous-local init --database-mode managed
corepack pnpm nous --instance ../temp/nous-local runtime acquire postgres
corepack pnpm nous --instance ../temp/nous-local auth init --email admin@example.test
corepack pnpm nous --instance ../temp/nous-local host start
```

执行`auth init`前在当前终端提供`NOUS_ADMIN_PASSWORD`。也可用`init --database-mode external --database-url-env NOUS_DATABASE_URL`连接已有PostgreSQL18。初始化不启动Subject或模型。

另一终端可执行以下管理命令；`doctor`在Host未启动时也可用：

```bash
corepack pnpm nous --instance ../temp/nous-local doctor
corepack pnpm nous --instance ../temp/nous-local status
corepack pnpm nous --instance ../temp/nous-local subjects create Demo
corepack pnpm nous --instance ../temp/nous-local settings schema
corepack pnpm nous --instance ../temp/nous-local providers list
corepack pnpm nous --instance ../temp/nous-local roles list
corepack pnpm nous --instance ../temp/nous-local prompts catalog
corepack pnpm nous --instance ../temp/nous-local models invocations list
corepack pnpm nous --instance ../temp/nous-local host stop
```

Provider、Model、Role与Prompt分别管理。职责默认禁用，显式配置资源、权限与预算后再调用。各命令的JSON输入和参数由`--help`及同一OpenAPI说明；`pnpm generate`通过固定上游生成器更新SDK。独立包先在`temp/releases/infrastructure/`验证，正式发行状态见施工记录。

Windows与macOS采用相同CLI/逻辑目录合同，平台安装验证继续按施工顺序完成；Windows开发临时根为`C:\dev\Heptalogos-Devs\temp\`。标准`dist/`、`node_modules/`和已有`data/`研究资料保留各自位置。
