# 工具链与开发环境

## Python 与发行单元

主应用使用 CPython 3.14 的标准 GIL 构建。它处于正常维护期，已有成熟 typing 与异步设施，生态有充分时间适配；暂不采用新近发布分支、自由线程构建或实验 JIT作为工程前提。用 `.python-version` 指定 `3.14`，初始化环境时由 uv 安装可用补丁版本。

已有 `sdk/python` 当前发行名为 `nous-wave`、导入名为 `nous_wave`，最低 Python 3.11。本轮保留此边界。应用发行名使用 `nous-wave-server`，导入名 `nous`；native 发行名 `nous-wave-native`，导入名 `nous_native`。客户端只依赖 HTTP、公共合同及序列化，不要求安装原生计算。

uv workspace 包含根主应用、`sdk/python`、`native/python`，共享根 `uv.lock`。运行应用时安装包含 native 的工作区；只开发 SDK 时选择 SDK 包。配置根 pyproject 的 workspace source，使用 editable 安装，避免手改 `PYTHONPATH` 或复制包进 site-packages。

仓库根目录已有 `uv.lock`，日常使用 `uv sync --locked`。新增依赖时只解析受影响的变更；实际版本以锁文件为准。DBOS 使用稳定 3.x。Rust 使用仓库锁定的工具链，PyO3/rust-numpy/maturin 使用彼此支持的稳定组合并写入 native Cargo.lock。

## 强类型是一条日常开发路径

采用 **Basedpyright strict** 作为唯一 Python 静态类型检查器。它沿用 Pyright 的分析能力并提供额外诊断，可作为 Python 开发依赖安装，也提供 `basedpyright-langserver`。不再叠加 Mypy、Pyright、Pylance 和另一套类型检查输出。

执行入口：

```bash
uv run basedpyright
uv run ruff check src sdk/python/src native/python/python
uv run ruff format src sdk/python/src native/python/python
```

类型规则：

- 对外函数、业务命令、结果、Provider 输入输出、原生边界全部标注类型；容器标注元素类型。
- 外部 JSON 先作为 `object` 或 SDK 明确类型接收，使用 Pydantic/TypeAdapter 在边界解析，内部使用确定类型。
- 核心对象按语义定义，例如 SubjectId、ObservationRef、TemporalValue、CognitiveRevision、EmbeddingSpace；避免通用 `dict[str, Any]` 贯穿系统。
- Pydantic 用于输入、配置和公开记录；领域计算的私有中间对象可用冻结 dataclass。能复用同一正式记录时直接使用，避免额外 DTO 转换链。
- `.pyi` 为 `nous_native._core` 声明完整构造、方法、异常、数组 dtype 与返回类型，包携带 `py.typed`。
- 原生第三方 SDK 的无类型响应只在其接入模块内归一化。需要忽略类型时精确到诊断和局部行，并说明该实际 API 的类型缺口。不能把整个目录排除以绕开尚未建好的接口。
- Python 类型检查与运行时校验各自承担责任。CLI 文本先解析成实际 UUID/日期/枚举，再交给严格业务命令；JSON 网络输入使用对应 JSON 验证入口，不靠松散转换吞掉格式问题。

根 `pyproject.toml` 配置 strict，额外禁止显式 Any 并检查未标注边界。第三方返回 Any 的消除以边界类型归一为主，不机械地为每个局部变量补注解。SDK 单独以 3.11 语法检查；应用和 native stub 以 3.14 检查。

## 编辑器、LSP 与 AI

跨编辑器默认用 `detachhead.basedpyright` 提供跳转、引用、签名与类型诊断；`python.languageServer` 设为 `None`，并拒绝 Pylance，避免与 Microsoft 语言服务叠诊。CLI 与 CI 只跑 `uv run basedpyright`，规则来自 `[tool.basedpyright]`。`charliermarsh.ruff` 负责格式与 lint；`rust-analyzer` 仅加载 `native/` workspace。Python 调试用 debugpy；测试发现启用 pytest（`tests/python`、`native/python/tests`）。

在 Cursor 里可用自带的 `anysphere.cursorpyright`（Basedpyright 分支）代替 basedpyright 扩展；二者只开其一，不要同时启用。其它 VS Code 兼容编辑器安装推荐的 basedpyright 扩展即可；CLI/格式/测试/调试配置相同，不依赖 Cursor。

根 `.vscode/` 指向 `temp/venvs/python-native`，含测试发现与 launch 配置。不要再叠加第二套类型检查输出或第二个格式器。

云端或其它 LSP 宿主可直接连接：

```bash
uv run basedpyright-langserver --stdio
uv run ruff server
```

只有 shell 工具的会话用 Basedpyright CLI 得到相同类型诊断；不要为了“AI 友好”再开发自制 LSP-to-MCP 服务。语言服务器作为常规工具提供符号信息，工作方式由真实宿主能力决定。

工程对 AI 的可读性主要来自：一个清楚的应用入口、一份依赖锁、确定的模块 owner、短而准确的公开操作、可定位的 Prompt、类型完整的 native stub、能直接运行的 CLI 和实际消费者示例。常驻 AGENTS 只记录这些当前工程事实，专项方法从已安装 AntiRLHF 按需读取。

## Ruff 与代码组织

Ruff 同时负责格式、导入排序和常见错误，选择 E4/E7/E9、F、I、B、UP、ASYNC 等实际有用规则，不启用全部风格规则。SDK 的语法升级规则按 3.11 目标处理。默认行宽 100；模块按职责拆分，手写代码文件控制在 600 行以内。

业务代码采用 async def 处理网络与等待，公开服务是显式类型化对象。默认不用元类、运行时魔术注册和动态字符串分派。注册只发生在真正的 Provider、任务处理器和功能扩展边界，普通模块用明确 import 和构造参数。

测试工具使用 pytest；异步测试优先使用 AnyIO 自带 pytest 支持。沿用原项目能够区分算法公式、真实数据丢失和来源泄漏的数值/业务案例，成组迁移；普通字段、私有 helper、语言功能和依赖内部行为不另写测试。项目不引入任意测试数量、覆盖率或重复复核门禁。

## Rust 构建范围与磁盘

新的 Rust workspace 放在 `native/`，只包含 `nous-vcp`、`nous-retrieval`、提取的 `nousql-syntax` 和 PyO3 入口。它有独立 Cargo.lock。旧根 Cargo workspace暂保留作参考，不作为 Python 日常安装、类型检查或 rust-analyzer 的构建目标。

开发循环：

```bash
source scripts/dev-env.sh
uv sync --locked
# 仅修改 Python 时直接运行；扩展源码变动后才运行下面命令。
uv run maturin develop --manifest-path native/python/Cargo.toml
uv run nous --help
```

编辑 native 源码后，uv 不保证现有进程已重新装载扩展；完成重编译后重启服务进程。Python 热重载也不得在旧 DBOS worker 未停止时创建第二个同实例 writer。

构建缓存统一进入 `temp/build/native`；uv、上游克隆、运行实例分别放入 `temp/cache/uv`、`temp/upstream`、`temp/instances`。保留一套有效缓存供后续任务复用。开发 profile 关闭大体积 debug 信息；发行时再启用优化。默认不做全工作区、全 feature、多个 target 的重复构建，不定期清空缓存强迫重编译。

`templates/native-workspace.toml` 给出独立 workspace 与 profile 起点。优化重点是减少需要重新编译的 Rust 范围和依赖图；不通过删除 native 算法能力换取构建速度。

## 安装与发行

日常用户可通过 uv 管理的隔离环境安装 `nous-wave-server`，其依赖选择匹配平台的 native wheel。主程序继续提供 `nous` 命令，程序目录与实例数据分离。开发机无需因 Python 主应用每次改动重新编译成 EXE。

native 使用 CPython 3.14 对应 wheel，针对 Windows x64、Linux x64、macOS 支持目标分别构建。NumPy 和 C++ 依赖加入 ABI 与 wheel 选择考量，不在未确认时宣称 abi3 一包通用。本机 Linux wheel 只适用于其平台标签；发行 wheel 使用 maturin/auditwheel 的 manylinux 规则。

将来需要免安装 Python 的桌面发行时，使用现成运行时分发或 Nuitka standalone 目录构建。该操作属于发行工作，不进入日常代码循环。云端 Linux 结果只报告实际运行平台；Windows/macOS 构建在对应 runner 或开发设备执行。
