# 临时数据

使用 temp 管理工具建立目录、查看状态和清理。命令从仓库根目录运行。

Linux 使用 [sh 入口](../../scripts/temp.sh)，需要 PATH 中有 Python 3.9+ 的 `python3`。脚本只用 Python 标准库，无需激活虚拟环境、安装项目依赖或 PowerShell：

```sh
sh scripts/temp.sh init
sh scripts/temp.sh status
sh scripts/temp.sh clean
sh scripts/temp.sh clean --apply
```

Windows 使用 [PowerShell 入口](../../scripts/temp.ps1)，需要 PowerShell 7（`pwsh`）：

```powershell
pwsh -File ./scripts/temp.ps1 init
pwsh -File ./scripts/temp.ps1 status
pwsh -File ./scripts/temp.ps1 clean
pwsh -File ./scripts/temp.ps1 clean -Apply
```

`init` 幂等补齐目录，不安装工具、不初始化实例；开发环境变量仍由 `scripts/dev-env.ps1` / `scripts/dev-env.sh` 设置。无参数默认 `status`；`status` 和不带 `--apply`（PowerShell 为 `-Apply`）的 `clean` 只预览。

| 目录 | 用途 |
| --- | --- |
| `temp/build/`、`temp/cache/` | 可重建的编译产物与依赖缓存 |
| `temp/runtime/` | 工具临时文件、pytest 临时目录 |
| `temp/work/<任务>/` | 一次性脚本、探针、实验输出和可丢弃实例；不散放在 temp 根目录 |
| `temp/reference-code/` | 可重新拉取的参考仓库与临时源码 |
| `temp/tools/`、`temp/venvs/` | 本地工具、下载的运行时和虚拟环境 |
| `temp/test-materials/` | 需要保留的输入材料、演示实例及可复查结果 |

测试材料按 `Audio/`、`Image/`、`PDF/`、`Text/`、`Video/` 分类；来源说明、授权和文稿跟随对应素材。配置、模板、约束和集合清单分别放 `Config/`、`Prompts/`、`Schemas/`、`Collections/`；运行结果、导出和诊断放 `Results/`、`Exports/`、`Diagnostics/`。保留的测试实例放 `test-materials/Instances/<名称>/`，实际创建使用 Nous 的 `init` 命令。清理时从旧散落目录救出的素材先放 `test-materials/Incoming/`，后续归类并修正本地引用。

`clean --apply` 清理 `test-materials/` 之外的旧实例、参考源码、缓存、构建及临时工作，默认保留 `tools/`、`venvs/`，随后补齐空标准目录。`--include-toolchains` 同时清理工具和虚拟环境，使用后需重新安装；PowerShell 对应参数为 `-Apply`、`-IncludeToolchains`。sh 入口也接受这两个 PowerShell 风格参数。

候选子树任意层的 Git linked worktree 都保留其整个祖先候选，请使用 `git worktree remove` 处理。运行中进程引用或容器 bind mount 涉及的候选会跳过；符号链接候选也会跳过，子树中的普通符号链接只删除链接本身。每次实际删除前重新检查子树、进程和挂载。样本复制必须通过校验，目标已有不同内容时停止清理。

Linux 使用 `/proc` 的命令行、工作目录及可读取的打开文件，命令行相对路径按该进程工作目录解释。sh 版的实际清理仅支持 Linux，无法取得必要的进程或挂载信息时拒绝执行；预览仍可使用，并显示保留原因。PowerShell 在 Windows 使用 CIM 可取得的可执行路径与命令行，在 Linux 使用 `/proc`。

执行清理前，关闭 Nous、构建命令及其他使用临时目录的任务，把仍需保留的资料移入 `test-materials/`。sh 版使用清理锁阻止自身并发清理，并保留扫描后发生变化的候选。清理期间不要修改、替换或移动候选目录，也不要同时运行另一种清理工具；进程检查和文件复查不提供与其他写入操作之间的事务隔离。不得用清理工具操作仓库外或当前任务未授权的数据。

完成工作后清理不再需要的临时脚本、测试产物和废弃构建。`temp/` 已被 Git 忽略；长期有用的产品示例、正式脚本和回归测试应迁入对应受版本管理的目录。
