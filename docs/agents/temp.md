# 临时数据

使用 [temp 管理工具](../../scripts/temp.ps1) 建立目录、查看状态和清理。需要 PowerShell 7（`pwsh`），命令从仓库根目录运行：

```powershell
pwsh -File ./scripts/temp.ps1 init
pwsh -File ./scripts/temp.ps1 status
pwsh -File ./scripts/temp.ps1 clean
pwsh -File ./scripts/temp.ps1 clean -Apply
```

`init` 幂等补齐目录，不安装工具、不初始化实例；开发环境变量仍由 `scripts/dev-env.ps1` / `scripts/dev-env.sh` 设置。无参数默认 `status`；`status` 和不带 `-Apply` 的 `clean` 只预览。

| 目录 | 用途 |
| --- | --- |
| `temp/build/`、`temp/cache/` | 可重建的编译产物与依赖缓存 |
| `temp/runtime/` | 工具临时文件、pytest 临时目录 |
| `temp/work/<任务>/` | 一次性脚本、探针、实验输出和可丢弃实例；不散放在 temp 根目录 |
| `temp/reference-code/` | 可重新拉取的参考仓库与临时源码 |
| `temp/tools/`、`temp/venvs/` | 本地工具、下载的运行时和虚拟环境 |
| `temp/test-materials/` | 需要保留的输入材料、演示实例及可复查结果 |

测试材料按 `Audio/`、`Image/`、`PDF/`、`Text/`、`Video/` 分类；来源说明、授权和文稿跟随对应素材。配置、模板、约束和集合清单分别放 `Config/`、`Prompts/`、`Schemas/`、`Collections/`；运行结果、导出和诊断放 `Results/`、`Exports/`、`Diagnostics/`。保留的测试实例放 `test-materials/Instances/<名称>/`，实际创建使用 Nous 的 `init` 命令。清理时从旧散落目录救出的素材先放 `test-materials/Incoming/`，后续归类并修正本地引用。

`clean -Apply` 清理 `test-materials/` 之外的旧实例、参考源码、缓存、构建及临时工作，默认保留 `tools/`、`venvs/`，随后补齐空标准目录。`-IncludeToolchains` 同时清理工具和虚拟环境，使用后需重新安装。检测到的运行中进程、容器挂载、链接路径及 Git linked worktree 会跳过。候选子树任意层的 linked worktree 都保留其整个祖先候选；每次实际删除前重新检查子树、进程和挂载。Linux 使用 `/proc` 的命令行、工作目录及可读取的打开文件，命令行相对路径按该进程工作目录解释；Windows 使用 CIM 可取得的可执行路径与命令行。当前用户进程的必要信息缺项时，无法确认安全的候选会保留并说明，预览仍可使用。执行清理前，把仍需保留的资料移入 `test-materials/`；不得用清理工具操作仓库外或当前任务未授权的数据。

完成工作后清理不再需要的临时脚本、测试产物和废弃构建。`temp/` 已被 Git 忽略；长期有用的产品示例、正式脚本和回归测试应迁入对应受版本管理的目录。
