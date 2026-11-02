# 安装 Python 服务与 native 扩展

主应用、客户端与原生计算分别是 `nous-server`、`nous-sdk`、`nous-native`。服务使用标准 CPython 3.14，包含 GIL；独立 SDK 仍支持 Python 3.11 及以上。用户安装匹配平台的 native wheel，无需 Rust、C++、Node 或旧 Rust 服务。

## 安装发行包

从同一次构建取得对应平台的完整 `nous-cp314-<target>` 产物，解压后应包含三个同版本 wheel 和 `install-python.py`。Linux x64、Windows x64、macOS Apple Silicon、macOS Intel 分别使用自己的产物。不要混合不同发行版本。

已安装 uv 时，在解压目录运行：

```bash
uv run --python 3.14 --no-project install-python.py --wheels .
```

已有标准 CPython 3.14 时也可运行 `python install-python.py --wheels .`；脚本优先使用 uv，否则使用标准 `venv` 与 pip。三个项目 wheel 在一次依赖解析中安装，强制使用二进制分发；平台不匹配时安装失败，不会偷偷编译 Rust。普通第三方依赖从包索引下载，完整离线 wheelhouse 可加 `--offline`。

安装目录和实例目录独立，可显式指定：

```bash
python install-python.py --wheels . --venv /path/to/venv --instance /path/to/instance
```

默认实例位于当前用户的数据目录：Linux 为 `${XDG_DATA_HOME:-~/.local/share}/nous/instance`，macOS 为 `~/Library/Application Support/Nous/instance`，Windows 为 `%LOCALAPPDATA%\Nous\instance`。直接运行 `nous init`、`nous serve` 和安装器使用同一默认位置，更换当前工作目录不会切换实例。`NOUS_INSTANCE` 可指定实例路径，命令行 `--instance` 优先。新数据目录不存在而旧 `NousWave`／`nous-wave` 目录已有实例时，继续使用旧实例。程序安装到独立的 `venv-nous` 环境，已有实例和管理凭据保持原位。已有服务仍使用其原先注册的程序路径；需要切换服务程序时按下方服务管理说明重新注册。凭据正文留在 `<instance>/credentials/admin.token`，不要写入命令日志或提交仓库。

安装器会输出适用于当前系统的完整启动命令：

```bash
/path/to/venv/bin/nous serve --instance /path/to/instance --host 127.0.0.1 --port 7441
```

Windows 对应 `C:\path\to\venv\Scripts\nous.exe`。通过同一安装的 `nous --help` 查看 CLI；HTTP 管理界面与 API 使用该实例的凭据。实例数据库、内容、索引和后台任务均随实例保存，程序升级不需要移动它们。

音视频及 GIF 动画准备另需 `ffmpeg` 和 `ffprobe`。Debian/Ubuntu 可安装 `ffmpeg`，macOS 可使用 Homebrew 的 `ffmpeg`，Windows 可安装 FFmpeg 并将其 `bin` 目录加入 PATH，也可在配置页的 `media` 中填写两个程序的完整路径。该配置仅保存到实例，包含像素、帧数、派生文件大小、进程并发与超时限制；后续媒体操作读取新值，已开始的操作保留自己的参数。静态图片由 Pillow 处理，PDF 由 pypdf 处理。

GIF 保留原件及帧时长。整段或时间、像素选区会准备为视频供模型读取；精确帧采样返回相应时间位置的合成画面。需要保留派生材料时使用 `media derive`，结果可继续按原件和选区追溯。

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

`namespace page`、`subject page NAMESPACE_ID` 支持 `--limit`、`--cursor`；输出的 `next_cursor` 用于下一页。`--purpose management` 需要管理权限，并包含已停用和回收站中的身份。`namespace change NAMESPACE_ID command.json`、`subject change NAMESPACE_ID SUBJECT_ID command.json` 接受 `rename`、`enable`、`disable`、`trash`、`restore`，可加 `expected_revision` 防止覆盖并发修改。停用和移入回收站会阻断该身份的普通操作；恢复保留原身份与数据，并保留停用状态。

`subject capabilities NAMESPACE_ID SUBJECT_ID` 区分服务支持、主体配置和当前启用的能力。修改用 `{"action":"configure_capabilities","capabilities":["memory","ego"]}`；支持 `memory`、`ego`、`social`、`motivation`。`subject bind NAMESPACE_ID command.json` 接受 `external_id` 及可选的现存 `subject_id`；宿主按自身实际稳定标识绑定。相同调用者、Namespace、外部标识会继续取得同一 Subject，凭据轮换保留该绑定。省略 `subject_id` 时首次创建，之后读取可用 `subject binding NAMESPACE_ID EXTERNAL_ID`。

`namespace export NAMESPACE_ID archive.json` 保存该 Namespace 的语义 Subject、全部认知修订、对应原件、派生、向量、形成调用记录及身份绑定和作用域配置；`namespace restore NEW_NAMESPACE_ID archive.json OPERATION_ID` 恢复到新 Namespace。此归档范围是 `semantic_subjects`；完整工作状态使用 Namespace 备份或实例备份保存。语义归档的导出与恢复需要完整实例管理权限。`subject migrate SOURCE_NAMESPACE_ID SOURCE_SUBJECT_ID TARGET_NAMESPACE_ID OPERATION_ID` 在目标中创建接收 Subject，返回来源对象与接收对象的映射，原 Subject 保留。

`namespace backup NAMESPACE_ID --output namespace.nousbackup` 创建该 Namespace 的一致性 ZIP 备份，包含 Context 工作状态、待办、作业检查点、未归属 Subject 的受管资料和对应的实际文件。`namespace backups NAMESPACE_ID` 分页查询，`namespace backup-info BACKUP_ID` 查看元数据。上传到另一运行实例用 `namespace backup-upload namespace.nousbackup`，再用 `namespace backup-restore NEW_NAMESPACE_ID BACKUP_ID OPERATION_ID` 创建副本。副本分配新身份，返回对象映射；为宿主授予接收 Subject 的凭据范围并完成绑定后继续使用。未完成作业暂停，并保留其来源及恢复状态；副本工作通过新的领域命令继续。`namespace backup-delete BACKUP_ID` 删除对应备份文件。

安装恢复可在没有原 Namespace 的目标实例中使用 `namespace backup-restore ORIGINAL_NAMESPACE_ID BACKUP_ID OPERATION_ID --preserve-ids`，保留原对象身份和调用绑定；确定的旧调用结果由历史托管提供。无法还原身份的旧调用绑定会列入 `unresolved_invocations`，相应待办保持暂停。Namespace 备份使用 `nous-namespace-backup` 格式，分块接口每块最多 1 MiB，偏移与清单、文件大小、哈希均校验。上传与恢复需要完整实例管理权限；备份目录与下载、删除遵循来源 Namespace 的管理授权。

`source` 提供目录、读取、预览、下载、移入回收站、恢复和清除；修改来源时使用 `--reason`，预览得到的摘要可传 `--preview`。`catalog`、`cognition`、`media`、`bundle`、`context` 的结构化命令接受对应 SDK 公共合同的 JSON 文件；文件路径 `-` 表示从标准输入读取。各组 `--help` 列出实际参数。上下文准入和反馈应填写宿主实际测量的数据。

`source preview NAMESPACE_ID SUBJECT_ID --request selection.json --action purge` 接受 `SourceSelection`：`source_names` 选择真实来源名，`observation_ids` 选择接触事件，`references` 选择带修订的认知对象，`records` 使用现有 `RecordsQuery`。目录的 `text` 做正文包含匹配，`text_match=label` 做 Entity 名称匹配。预览返回相关范围及 token；`source manage NAMESPACE_ID SUBJECT_ID trash|restore|purge command.json` 接受同一选择和 `SourceCommand` 的 `reason`、`operation_id`、可选 `expected_preview`、`force`、`grace_hours`。结果的 `retired`、`restored` 是本次提交真实失去或恢复可用性的对象。

`namespace purge NAMESPACE_ID command.json` 永久清除已在回收站中的 Namespace。它移除该范围的托管备份、作业历史、正文及派生状态，释放内容引用；仍被其他范围引用的文件继续保留。清除中的身份阻断并行恢复，已清除的 Namespace 和 Subject 标识保留退役记录。

`jobs` 查询、等待、取消与恢复持久任务；`config`、`prompts` 管理配置与提示词。`credentials create command.json --token-out /path/to/new-key` 以独占方式创建凭据文件，已有文件会拒绝覆盖；省略 `--token-out` 时只在创建响应显示一次凭据。作用域内的管理凭据可管理被授予范围内的密钥与配置，完整实例备份、全局提示词和模型资源修改需要完整实例管理权限。

`credentials whoami` 显示当前调用者及授权范围；`credentials read ID` 和 `credentials history ID` 查看密钥的当前修订和生命周期事件。轮换使用 `credentials rotate ID command.json --token-out /path/to/replacement-key`，JSON 包含 `expected_revision`、可选 `operation_id` 和 `reason`。旧密钥立即失效，授权范围保持一致；相同操作重试只返回元数据。保存替代密钥后将调用方的凭据文件引用改为新文件。

创建凭据可填写带时区的 `expires_at`，例如 `"2027-01-01T00:00:00Z"`；到期后请求返回 `credential_expired`。轮换保留原到期时间。

`nous bundle export bundle.json` 将当前 Namespace 的认知、来源和原件导出到文件；切换目标 Namespace 后用 `nous bundle install bundle.json` 安装。`nous bundle list` 列出安装记录，`nous bundle uninstall INSTALLATION_ID` 撤回该安装拥有的来源。

```bash
nous --instance /path/to/instance backup --output nous-backup.zip
nous restore nous-backup.zip /path/to/new-instance
```

恢复只接受新的目标实例；恢复后的管理凭据由新实例初始化生成。

## HTTP、SDK 与 MCP

HTTP 接口和 OpenAPI 文档位于运行服务的 `/docs`。读取当前 Ego 使用 `GET /api/namespaces/NAMESPACE_ID/subjects/SUBJECT_ID/ego/profile`；`ego/profile/view` 返回含 `markdown` 的 JSON，`ego/profile/markdown` 返回 Markdown 正文。请求使用 `Authorization: Bearer TOKEN`，授权范围由凭据及 Source 约束共同决定。

```python
import asyncio
from pathlib import Path
from nous_sdk import NousClient

async def main():
    async with NousClient("http://127.0.0.1:7441", credential_file=Path("/path/to/key")) as api:
        print(await api.ego_markdown("NAMESPACE_ID", "SUBJECT_ID"))

asyncio.run(main())
```

MCP 使用标准输入输出，连接已有 HTTP 服务：

```bash
nous --url http://127.0.0.1:7441 --credential-file /path/to/key mcp --namespace-id NAMESPACE_ID --subject-id SUBJECT_ID
```

`current_personality` 读取当前人格，`read_maintenance` 读取指定提案，`motivation_assessment` 与 `motivation_assessments` 读取已保存评价；这些读取操作使用已存状态。

## 种子提案与评价历史

种子解释和成长提案按依赖分组保存。读取运行结果可见原始动作、逐组错误、来源目录和已采用的精确修订。`ReviewMaintenance.group_keys` 可选择分组，所需前置分组一同采用；独立失败组继续保留。截断输出须逐组人工审阅。

```bash
nous maintenance read NAMESPACE_ID SUBJECT_ID RUN_ID
nous maintenance capture-output NAMESPACE_ID SUBJECT_ID RUN_ID capture.json
nous maintenance edit NAMESPACE_ID SUBJECT_ID RUN_ID edit.json
nous maintenance review NAMESPACE_ID SUBJECT_ID RUN_ID adopt REVISION --group GROUP_KEY
```

`capture.json` 使用 `CaptureMaintenanceOutput`，包含 `operation_id`、`expected_revision`、`call_id`；它读取该运行已有模型响应。`edit.json` 使用 `EditMaintenanceProposal`，包含 `operation_id`、`expected_revision`、`proposal` 和 `reason`。原始模型调用保持原状态，人工编辑记录操作者和理由。已采用动作保留；追加具有新 `key`、`target_key` 指向已采用键的动作，可修订同一认知身份。工作台保留提案和分组读取、采用与拒绝；编辑通过 CLI、HTTP、SDK 或 MCP 完成。

当前人格可用 `nous ego read NAMESPACE_ID SUBJECT_ID --output persona.md` 导出。Windows 发行目录的 `examples/literature-ego/ego.md` 是实际文学实例的系统输出：四项 Ego 认识；`profile.json` 保留对应完整记录，`records.json` 保留包含人物身份和病情自述记忆在内的六项当前认识。`source-resource.json`、`source-catalogs.json` 与 `hongloumeng-chapter-003-oldid2703828.html` 保留原第三回固定版本和精确来源。示例用于阅读，不会由安装器导入实例。

人物 Entity 的 `description` 和 `aliases` 保留来源认识，`generated_profile` 保存人物画像的自动摘要及其精确证据。SDK 的 `profile_text`、`effective_aliases` 给出当前有效内容。刷新只有原描述可用的人物会保留该描述，并跳过模型。证据撤回后，派生画像从当前读取中移除，仍有独立来源的描述和别名继续可用；已准备的旧 Context 会要求重新准备。

`AssessMotivation` 默认即时返回评价。显式传入 `save: true` 和固定 `operation_id`，会保存原 Desire/Concern 精确修订、原条件、证据、评价时点、区间覆盖与空缺、三值结果和模型判断。返回的 `assessment_id` 可读取完整保存记录。同一操作重放复用原结果。

```bash
nous assess-motivation NAMESPACE_ID SUBJECT_ID assessment-request.json
nous motivation-assessment NAMESPACE_ID SUBJECT_ID ASSESSMENT_ID
nous motivation-history NAMESPACE_ID SUBJECT_ID history-query.json
```

`history-query.json` 使用 `MotivationAssessmentsQuery`，可含 `reference`、`limit`、`cursor`。SDK 对应 `assess_motivation`、`motivation_assessment`、`motivation_assessments`，MCP 同名。Context 为选中的 Desire/Concern 携带同一精确修订最近保存的评价；准备 Context 时读取已有结果。条件产生新修订后，旧评价继续留在原修订历史中。来源永久清除同时清除相关评价与缓存正文；Namespace 备份包含评价及其精确引用。

来源和对象管理的预览会返回 `retention_states`，包含所选 Memory/Association 的实际修订、强度、时间锚点、非活跃原因、永久与保护状态。将其中所需状态去掉动态 `score`，作为 `SourceCommand.retention_expectations` 提交，可阻止过期计划修改已经变化的对象。执行事务重新读取并授权，变化返回 `retention_precondition_failed`。

## 检索调优

`tuning create NAMESPACE_ID SUBJECT_ID request.json` 接受 SDK `CreateTuning` 合同：`cases` 包含已有查询的 `id`、`query` 和 `relevance`（精确引用到非负相关性等级）；`parameters` 指定当前算法参数与候选值，也可用 `profiles` 提供完整配置。`split=holdout` 的查询在开发集选优后单独运行。默认预算包含原配置，最多 8 个候选；增加 `max_trials` 才扩大预算。接口保留每条查询的排名、指标、延迟、覆盖和降级状态。

```bash
nous tuning create NAMESPACE_ID SUBJECT_ID request.json
nous tuning get NAMESPACE_ID SUBJECT_ID TASK_ID
nous tuning rounds NAMESPACE_ID SUBJECT_ID TASK_ID
nous tuning profile NAMESPACE_ID SUBJECT_ID
nous tuning apply NAMESPACE_ID SUBJECT_ID selected-profile.json
nous tuning rollback NAMESPACE_ID SUBJECT_ID rollback.json
```

创建操作返回实际 `job_id`，用 `jobs wait` 等待；`tuning cancel` 取消未完成工作。`ApplyRetrievalProfile` 可指定完整 `profile` 或已完成的 `task_id`。试验不改变正式配置；应用和回滚各产生新的配置修订，可携带 `expected_commit_seq` 检查并发变更。SDK 提供同名 typed 操作。

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
uv build --package nous-server --sdist --wheel --no-sources --out-dir dist
uv build --package nous-sdk --sdist --wheel --no-sources --out-dir dist
uv build --package nous-native --sdist --no-sources --out-dir dist
uv run maturin build --locked --release --manifest-path native/python/Cargo.toml --interpreter python3.14 --out dist
python scripts/install-python.py --wheels dist --venv /tmp/nous-installed --instance /tmp/nous-instance
```

构建使用 `native/python/Cargo.toml` 和 `native/Cargo.lock`。Windows 源码构建需要 Visual Studio C++ Build Tools、Windows SDK 与 CMake；macOS 需要对应架构的 Xcode Command Line Tools。Python 依赖使用根 `uv.lock`，第三方源码和构建缓存留在 `temp/`。

`.github/workflows/python-native-release.yml` 支持手动 `workflow_dispatch`；当前开发分支 `codex/python-native-rewrite-cloud` 上仅修改该 workflow 文件的 push 也会触发构建。它构建三个包的 sdist、server/SDK 通用 wheel，以及四个平台的 native wheel；每个平台再安装完整三件套、初始化实例并实际启动 HTTP 服务。产物保存在 Actions artifacts，不发布包索引。

Linux 发布构建使用 manylinux 2.28 容器。当前本机 Ubuntu 构建得到的 `manylinux_2_39_x86_64` wheel 需要 glibc 2.39 或更新版本，不能当作支持旧 Linux 的通用包。Windows/macOS runner 配置是构建路径，运行结果以对应 Actions 任务为准。

native 分发保留已有 `nous_native/licenses/` 中的 Apache 文本及 VCP 的 `CC-BY-NC-SA-4.0` 文本，其限制适用于对应采用代码。server/SDK 尚未声明整体许可证。
