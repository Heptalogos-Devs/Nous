# 参考项目检索

涉及功能实现和设计取舍时，针对当前问题检索相关参考项目的实际代码，阅读上下文，再结合 Nous 的领域语义、接口和依赖决定采用方式。引用或改编代码时保留所需的许可与来源。

参考仓库位于 `temp/reference-code/`。XERJ 是本地词法检索工具：使用 MCP `xerj_search`，按下表选择 `prefix`，使用源码中的英文术语或符号查询。未克隆仓库自行拉取。

| 前缀 | 参考项目 | 适用问题 |
| --- | --- | --- |
| `refamemorix` | A-Memorix | 记忆与管理合同、画像、Episode、导入与 Bundle |
| `refmaibot` | MaiBot | 实际消费路径、插件、消息、模型和 Dashboard |
| `refvcp` | VCPToolBox | VCP、RiverMemo、检索与记忆实现 |
| `refmem0` | Mem0 | 长期记忆形成、检索与集成 |
| `refgraphiti` | Graphiti | 知识图谱、时间、实体及关系检索 |

已有 XERJ 配置的 MCP 注册名为 `xerj`，服务地址为 `http://127.0.0.1:19320`。没有 MCP 时可用 CLI。索引过期时，在更新对应参考仓库后以相同前缀重新建索引。以下命令适用于已经按此约定安装的环境；没有对应工具、服务或参考仓库时，改用可用检索方式。

Linux 的已有配置：程序为 `~/.local/bin/xerj`，索引位于 `~/.local/share/xerj/nous-reference/data`。已有服务停止时可执行 `systemctl --user start xerj-nous.service`，不因此新建自启动服务。

```bash
~/.local/bin/xerj search --url http://127.0.0.1:19320 --prefix refamemorix 'memory query retrieval'
~/.local/bin/xerj autoindex temp/reference-code/A_memorix --url http://127.0.0.1:19320 --prefix refamemorix --workers 2 --no-graph --no-semantic --code-analyzer code --max-minutes 0
```

Windows 的已有配置：程序为 `%LOCALAPPDATA%\Programs\xerj\xerj.exe`，索引位于 `%LOCALAPPDATA%\xerj\nous-reference\data`。服务未运行时，先启动本地进程，再检索或重建索引：

```powershell
$xerj = Join-Path $env:LOCALAPPDATA 'Programs\xerj\xerj.exe'
$data = Join-Path $env:LOCALAPPDATA 'xerj\nous-reference\data'
Start-Process -WindowStyle Hidden -FilePath $xerj -ArgumentList @('--insecure', '--bind', '127.0.0.1', '--port', '19320', '--data-dir', "`"$data`"", '--disable-feedback')

& $xerj search --url http://127.0.0.1:19320 --prefix refamemorix 'memory query retrieval'
& $xerj autoindex temp/reference-code/A_memorix --url http://127.0.0.1:19320 --prefix refamemorix --workers 2 --no-graph --no-semantic --code-analyzer code --max-minutes 0
```
