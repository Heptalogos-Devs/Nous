# temp 管理工具

在仓库根目录运行。入口将参数交给同一个 Python 脚本；需要 Python 3.9 或更高版本，使用标准库，无需安装项目依赖。Windows 入口需要 PowerShell 7 和 PATH 中的 `python`；Linux 入口需要 PATH 中的 `python3`。

```powershell
pwsh -File ./scripts/temp/run.ps1 init
pwsh -File ./scripts/temp/run.ps1 status
pwsh -File ./scripts/temp/run.ps1 clean
pwsh -File ./scripts/temp/run.ps1 clean -Apply
pwsh -File ./scripts/temp/run.ps1 clean temp/work/example-task
pwsh -File ./scripts/temp/run.ps1 clean temp/work/example-task -Apply
```

```sh
sh scripts/temp/run.sh init
sh scripts/temp/run.sh status
sh scripts/temp/run.sh clean
sh scripts/temp/run.sh clean --apply
sh scripts/temp/run.sh clean temp/work/example-task
sh scripts/temp/run.sh clean temp/work/example-task --apply
```

`init` 补齐目录，并在缺失时根据模板创建 `temp/AGENTS.md`；已有文件保持原样。无参数默认为 `status`。`status` 和不带 `--apply` 的 `clean` 只预览；`--include-toolchains`（或 `-IncludeToolchains`）将工具和虚拟环境加入清理范围。两种入口接受相同的参数。

`clean` 后可指定一个或多个 `temp/` 内的文件或目录；接受相对仓库根目录、相对 `temp/` 或绝对路径。指定路径时只处理这些目标，`temp/test-materials/`、`tools/` 和 `venvs/` 的子项也可以明确选择，不需要 `--include-toolchains`。重叠目标只处理一次；路径不存在、位于 `temp/` 外，或其祖先为链接时停止，目标是链接时保留。不能选择 `temp/`、`temp/test-materials/`、清理锁或 `temp/AGENTS.md`。目标包含 Agent 指令或 `test-materials/` 中以 `0-` 开头的用户原始材料时予以保留。

`init` 不安装工具或初始化实例；开发环境与检索工具由 `dev.ps1` 和 `dev.sh` 设置。`temp/work/<任务>/` 存放一次性工作；`test-materials/` 保留输入材料、结果和测试实例，目录分类与素材约定见生成的 `temp/AGENTS.md`。用 Nous 的 `init` 命令创建需要保留的实例。

不指定路径的清理默认保留 `AGENTS.md`、`test-materials/`、`tools/` 和 `venvs/`，完成后补齐标准目录；指定路径时不会重建被清理的目标。指定 `--include-toolchains` 后需要重新安装被清理的工具和虚拟环境。散落的输入素材会先复制到 `test-materials/Incoming/` 并校验；目标已有不同内容时停止清理。归集后按用途分类并修正本地引用。

选择 `test-materials/` 中的具体项目代表明确清理该项目，其中内容不会再复制到 `Incoming/`；其他位置的散落素材仍按上述规则保留。工具按扫描得到的文件逐项删除，再移除空目录；只处理预览所列目标，不运行递归删除命令。

清理前关闭使用相关目录的任务，把所需资料放入 `test-materials/`。运行中进程引用、挂载和 Git linked worktree 涉及的候选会保留；使用 `git worktree remove` 处理 linked worktree。候选路径是符号链接时会保留，子树中的普通链接只删除链接本身。清理前会重新检查候选、进程和挂载；扫描后发生变化的候选会保留。

Windows 使用 CIM 获取进程的可执行路径和命令行，Linux 使用 `/proc` 检查命令行、工作目录及可读取的打开文件。Linux 命令行中的相对路径按进程工作目录解释。无法取得实际清理所需的信息时会停止；其他平台可以预览，但不执行清理。

清理锁阻止并发执行本工具。清理期间不要修改、替换或移动候选目录；进程检查和文件复查不提供与其他写入操作之间的事务隔离。不得操作仓库外或当前任务未授权的数据。长期有用的产品示例、正式脚本和回归测试应放入受版本管理的目录。
