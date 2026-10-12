# 文档检查与本地产物清理

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

| 文件 | 用途 |
| --- | --- |
| [check-doc-navigation.py](check-doc-navigation.py) | 检查本地链接、返回路径与 INDEX 覆盖 |
| [clean.ts](clean.ts) | 清理仓库 data/temp 和 data/cache，先停止遗留测试 cluster |

```text
python scripts/maintenance/check-doc-navigation.py
just clean-local
```

文档检查使用 Python 3.11+，读取 Git tracked 和非 ignored Markdown，报告失效本地链接、无返回链接、孤儿页面及最近祖先 INDEX 未收录的页面。README 和普通文档参与返回/覆盖检查；INDEX、AGENTS 不要求返回，`.agents/` Skills 和作为产品输入的 `prompts/` 不属于人类文档。文档需链接回收录它的目录或其他入口；INDEX 用明确的文件链接收录页面。忽略目录与豁免页面在 [.config/scripts/doc-navigation.toml](../../.config/scripts/doc-navigation.toml) 中配置，不需修改脚本；用 `--config <TOML路径>` 指定其他配置。目录和页面路径相对仓库根。发现问题退出 1，否则退出 0；这是按需工具。

`just clean-local` 是显式跨平台清理命令；先停止开发服务与测试再执行。它使用已安装 runtime 的 `pg_ctl` 停止 `data/temp/` 中遗留的测试、smoke 或发布验证 cluster，然后删除仓库 `data/temp/` 和 `data/cache/`。`NOUS_WAVE_POSTGRES_RUNTIME` 可指定安装位置；遇到符号链接或临时目录以外的 cluster 会在删除前报错。实例 Authority、配置、Secrets、真实研究与模型输出保留；Cargo `target/` 和 `node_modules/` 使用各工具的标准清理命令。
