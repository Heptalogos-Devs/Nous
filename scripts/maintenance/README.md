# 文档检查与临时 PostgreSQL 维护

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

| 文件 | 用途 |
| --- | --- |
| [check-doc-navigation.py](check-doc-navigation.py) | 检查本地链接、返回路径与 INDEX 覆盖 |
| [cleanup_embedded_postgres.ps1](cleanup_embedded_postgres.ps1) | 按明确标记和进程状态处理孤立测试 cluster |

```text
python scripts/maintenance/check-doc-navigation.py
powershell -NoProfile -File scripts/maintenance/cleanup_embedded_postgres.ps1 -WhatIf
```

文档检查使用 Python 3.11+，读取 Git tracked 和非 ignored Markdown，报告失效本地链接、无返回链接、孤儿页面及最近祖先 INDEX 未收录的页面。README 和普通文档参与返回/覆盖检查；INDEX、AGENTS 不要求返回，`.agents/` Skills 和作为产品输入的 `prompts/` 不属于人类文档。文档需链接回收录它的目录或其他入口；INDEX 用明确的文件链接收录页面。忽略目录与豁免页面在 [.config/scripts/doc-navigation.toml](../../.config/scripts/doc-navigation.toml) 中配置，不需修改脚本；用 `--config <TOML路径>` 指定其他配置。目录和页面路径相对仓库根。发现问题退出 1，否则退出 0；这是按需工具。

PostgreSQL 清理只处理`data/temp/tests/` 中具有 PostgreSQL cluster 标记的孤立测试根，跳过运行中的 PostgreSQL。省略 `-WhatIf` 执行删除；`-MinimumAgeHours <小时>` 限制最小年龄，默认 0。它不清理语料、手写配置或开发实例。
