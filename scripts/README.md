# Scripts 组织规范

除了工作区设定和管理，工具按下面的结构放置。

工作区设定是 `workspace.ps1`、`workspace.sh` 和 `workspace.py`。它们要写入当前 shell 的环境变量，在仓库根目录用 `. ./scripts/workspace.ps1` 或 `source scripts/workspace.sh` 进入当前会话。

## 结构

一个工具一个目录。入口和 Python 文件的名字由这个工具决定：

```text
scripts/<name>/
  README.md
  config/
    <配置文件>.toml
  <入口>.ps1
  <入口>.sh
  <程序>.py
  ...
```

`<程序>.py` 实现命令。配置、模板，以及按职责拆出的 Python 模块，放在同一目录。它以脚本方式运行，同目录模块按顶层名字导入。这个目录不是包，不加 `__init__.py`。仓库根目录取 `Path(__file__).resolve().parents[2]`。

`<入口>.ps1` 和 `<入口>.sh` 定位解释器，以 `-B` 运行同目录的 `<程序>.py`，原样转发参数，并返回 Python 的退出码。参数、配置和动作都在 Python 里。两个入口接受同一组参数；短选项在 Python 的参数解析中登记。

阈值、外部命令和扫描路径只留在该目录的配置文件中，由 Python 读取。`README.md` 写这个工具在仓库根目录的调用方式、参数和退出码。

```powershell
pwsh -File ./scripts/<name>/<入口>.ps1
```

```sh
sh scripts/<name>/<入口>.sh
```

Windows 入口需要 PowerShell 7 和 PATH 中的 `python`。Linux 和 macOS 入口需要 PATH 中的 `python3`。入口不改当前 shell 的环境，也不安装依赖。工具要使用项目虚拟环境时，调用者先做工作区设定，再由 `<程序>.py` 调用该环境中的解释器。
