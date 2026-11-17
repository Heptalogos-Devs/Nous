# 模型、媒体与内容技术决定

## 项目依据

本轮依据用户最新要求与仓库当前设计：统一核心、供应商中立、配置先行、真实多模态预实验、CAS/数据库接通、CLI 命令有正式归属，以及通路端验证。代码定位读取 main@429c841；不以旧 Spec 冻结未来架构。

用户明确授权本地 Docker New API 的直接接入与调用。官方文档可确认协议和管理方式，具体部署版本、可用模型与 Key 仍需在目标机器读取。本研究没有连接用户本地 Docker，也没有运行项目测试/CI。

## 采用的机制与工程决定

| 选择 | 官方依据 | 本项目采用方式 |
|---|---|---|
| Pydantic AI direct/model API | [Direct](https://pydantic.dev/docs/ai/core-concepts/direct/) | 执行明确请求，不引入自主 Harness |
| 明确 OpenAIChatModel 与 base URL | [OpenAI-compatible](https://pydantic.dev/docs/ai/models/openai/) | 只实现网关标准协议，避免依赖简写的默认路由 |
| 多模态消息类型 | [Input](https://pydantic.dev/docs/ai/core-concepts/input/) | 真实二进制输入，不把本地路径当远端 URL |
| Pydantic / JSON Schema 结果校验 | [Output](https://pydantic.dev/docs/ai/core-concepts/output/)、[jsonschema](https://python-jsonschema.readthedocs.io/en/stable/) | 同一合同验证输出；StructuredDict 的 Schema 转交能力不替代本地校验 |
| 文本 Embedder | [Embeddings](https://pydantic.dev/docs/ai/guides/embeddings/) | 相同网关连接，空间绑定在本项目 owner |
| Jinja | [API](https://jinja.palletsprojects.com/en/stable/api/) | StrictUndefined、loader、bytecode cache；请求使用冻结资源内容 |
| markdown-it-py | [Token map](https://markdown-it-py.readthedocs.io/en/latest/api/markdown_it.token.html) | 精确原文选段，无自制 Markdown 解析器 |
| Pillow | [Image](https://pillow.readthedocs.io/en/stable/reference/Image.html) | 图片与 GIF 的实际处理 |
| pypdfium2 | [API](https://pypdfium2.readthedocs.io/en/stable/python_api.html) | 页文本与栅格；其非线程安全限制由 PDF owner 的实际 worker 锁承担 |
| FFmpeg/ffprobe | [ffprobe](https://www.ffmpeg.org/ffprobe.html) | 流/时长/帧和转换；记录实际 PTS，seek 目标与实际结果分开 |
| New API | [起步](https://docs.newapi.pro/zh/docs)、[鉴权](https://docs.newapi.pro/zh/docs/api/management/auth) | 本次环境接入，不成为 Nous 产品依赖 |

依赖使用支持 CPython 3.14 的稳定版并写入现有 uv.lock，不为了缩减依赖数复制解析器、SDK、模板或媒体转换逻辑。原服务商协议差异留在网关与库，当前不展开全部提供商适配。

## 由本轮目标作出的选择

`nous tools` 是长期产品入口，临时开发脚本留 temp；本轮没有 dev 框架。CAS 与模型执行一同接通，DBOS 延后到真正需要持久恢复的消费者。Artifact/ProcessingResult 保存实际材料与派生，认知对象不提前引入。

三种媒体策略由一个应用 owner 编排。成功阶段保存并可复用；结果没有另一个任务状态机。请求预览与执行在同一准备结果上工作。参数、角色和来源由程序固定，模型完成专一语言工作。

协议/模态限制用实际准备与 coverage 表达。文本、媒体解码、JSON 校验、数据库采用有各自明确失败位置，不通过增加模型限制提示去补偿职责错误。
