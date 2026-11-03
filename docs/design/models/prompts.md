# Prompt 资源、快照与共同预览

## 资源

PromptCatalog 管理包内只读与实例可编辑的 Markdown/Jinja 资源。manifest 明确 contract_id、有序 message role/template 和 source 槽；模板只渲染纯值，不查数据库、不准备媒体、不发模型请求。

TaskDefinition 管理输入输出类型及引用规则。system/user/assistant 是本轮模板直接支持的角色；工具历史由 SDK 原生消息值保留。若新增 developer/tool 模板资源必须连同真实协议 ID 和返回配对完整接入，不能文档写支持而编译器丢弃。没有需要时保持准确的当前角色集合。

## 冻结范围

根操作开始时捕获其所有计划内任务资源，内容、manifest、literal include/extends 和输入输出合同共同确定版本。Jinja 使用 DictLoader 冻结本次实际内容、StrictUndefined、已配置编译缓存和 bytecode cache。后续阶段与 fallback 使用同一份已捕获资源。

输入正文只作为值插入一次，原文中的模板符号原样保留。动态文件名 include 本轮不支持。模板修改影响下一根操作；不建立每次编辑的永久版本数据库。

## 共同编译

同一 Compiler 供 preview、run、media 和后续认知任务使用。不要在 CLI 另建默认缓存参数的 Compiler。任务语义编译与协议适配分开：前者形成有序文本/媒体引用组，后者由成熟 SDK 提交实际媒体。

InputGroup 的文本和多个媒体 part 保持明确顺序；预览显示名称、类型、来源选区和已知尺寸，不打印 base64 或大矩阵。`append_sources` 是固定槽，没有过程式 DSL。

## 预览

纯模板 preview 可以接收准备好的文字和确定性媒体；其结果明确为模板预览，未选择实际模型时不宣称窗口/协议已检查。task preview 使用同一任务解析与表示选择，返回各批次分别的准备信息，不把所有批次拼成一个不可能实际发送的超大请求。

`model run --show-request` 展示即将执行的 PreparedGeneration 后直接执行同一对象；没有审批停顿。preview 不发转写/描述。缺少必需语言表示时明确指出由 media analyze 或正式 transcription 操作产生，或者用户显式提交已有结果。

## 编辑

导入目录与普通编辑共享资源加载及校验。先直接读取传入目录的 manifest 与 literal 依赖并校验，再写入实例覆盖。已发布请求持有旧内容，不依赖磁盘资源在执行中不变。

包内模板、实例覆盖和 schema 参数形成单一实际请求说明。采用结果保存实际模板版本；缓存只是性能资产。
