# 开发检查工具

在仓库根目录运行。先做工作区设定，再 `uv sync --locked`。阈值、外部命令和扫描路径在 [config/check.toml](config/check.toml)。

```powershell
pwsh -File ./scripts/check/run.ps1
pwsh -File ./scripts/check/run.ps1 length src/nous/content/results.py
pwsh -File ./scripts/check/run.ps1 tests -- tests/python/test_cli.py
pwsh -File ./scripts/check/run.ps1 tests --rust nous-vcp
pwsh -File ./scripts/check/run.ps1 all
pwsh -File ./scripts/check/run.ps1 --profile ci
```

```sh
sh scripts/check/run.sh
sh scripts/check/run.sh length src/nous/content/results.py
sh scripts/check/run.sh tests -- tests/python/test_cli.py
sh scripts/check/run.sh tests --rust nous-vcp
sh scripts/check/run.sh all
sh scripts/check/run.sh --profile ci
```

无参数的 `check` 运行长度、格式、lint 和类型，不运行测试。长度和格式都跑完；这一组有拒绝时，不再运行 lint、类型和测试。lint 和类型都跑完；其中有拒绝时，不再运行测试。警告不拒绝，也不跳过后续步骤。

`tests --` 把 `--` 后的参数交给 pytest。`--rust nous-vcp` 或 `--rust nous-retrieval` 只运行对应的 Rust 包。两者可以同时使用。`tests --all`、`all` 和 `-All` 运行配置中的全量 Python 与 Rust 测试。只写 `tests` 且没有目标时退出码为 2。

`length <文件>` 只计算这个文件的有效代码行，并打印行数。传入多个路径时，每个源码文件各打印一行。不带路径时检查配置中的根目录，只列出超限文件。

```text
src/nous/content/results.py  820  警告  > 800
length  警告  1
通过，警告 1
```

有效行数是只读格式化之后的代码行。Python 用 `ruff format --stdin-filename` 从 stdout 读取，Rust 用 `rustfmt --edition 2024 --emit stdout --config skip_children=true` 从 stdin 读取单个文件，避免输出文件名或递归展开子模块。这两个命令都不改写文件。空行、注释，以及模块、类、函数的文档字符串不计入。赋值用的多行字符串计入。Python 大于 800 警告、大于 900 拒绝；Rust 大于 500 警告、大于 600 拒绝。等于阈值仍然通过。

指定文件时，格式检查只接收这些路径。只有 Python 文件时不调用 rustfmt，只有 Rust 文件时不调用 ruff。

`--profile ci` 检查全树的 Python 长度、Python 格式、lint、类型和 `pytest tests/python`，不调用 rustfmt 或 cargo。`--profile hook` 只对传入文件做长度和格式。列表里的非 `.py`、`.rs` 文件跳过；没有任何源码文件时退出码为 0。不使用 hook profile 时，点名的文件都不是源码则退出码为 2。

CI 在安装依赖后运行：

```sh
sh scripts/check/run.sh --profile ci
```

pre-commit 可以只检查暂存文件的长度：

```sh
git diff --cached --name-only -z | sh scripts/check/run.sh --profile hook length --stdin --zero
```

```powershell
git diff --cached --name-only -z | pwsh -File ./scripts/check/run.ps1 --profile hook length --stdin --zero
```

`--stdin` 从标准输入读取路径，默认一行一个。`--zero` 改为以 NUL 分隔，并且必须和 `--stdin` 一起使用。没有 `--stdin` 时，工具不读取标准输入。仓库不安装 git hook。

退出码 0 表示通过，可以带有警告。退出码 1 表示有拒绝。退出码 2 表示用法或配置错误。
