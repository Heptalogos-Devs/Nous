# 模板、前缀与结果缓存

## 三类缓存分别维护

Jinja bytecode cache 复用模板编译，归 PromptCompiler。Provider 前缀缓存复用模型处理，归模型协议与请求组织。应用结果缓存复用完成的实际任务结果，归该结果的使用 owner。

缓存都不是正式认知权威。已采用正文和生成出处随业务保存，不因 cache 淘汰而丢失。

## 模板缓存

采用 Jinja 自带缓存和 FileSystemBytecodeCache，位置由 cache 逻辑根提供。模板、include 或编译配置变化产生新的编译结果。容量来自配置，不实现第二套模板 hash/文件监听系统。

## 前缀布局

优先将稳定的任务说明、必要例子、工具和输出合同保持稳定，把本次问题及变化材料放在后续合适角色中。无需为了缓存改变真实历史的消息顺序或混合角色。

cache breakpoint、cache key、TTL 等 Provider 细节留在适配层。不同模型支持的最小长度和失效规则可能不同，不在业务中硬编码一种模型的门槛。请求中的随机 ID、当前时间和调试标志只有任务确实需要时才进入文本，避免无意义地改变前缀。

运行记录保存实际 cached-input/write tokens（如提供方返回）及用量。应用 Session 连续只提供组织条件，实际命中由 Provider 报告。

## 结果复用

复用 key 至少包含真实任务、输入内容/精确修订、模板/输出合同、实际模型及影响结果的参数。存储在同一允许访问的范围内。权限或来源失效时先按当前正式来源检查。

已完成的媒体描述等可以复用。Dream、用户明确“重新生成”或需要新采样的任务允许绕过结果缓存。读取缓存不伪造一次新的模型生成或新的独立证据。

首轮模型交付首先实现模板缓存和实际 Provider 缓存参数/用量；通用任务结果缓存不先普遍开启。各正式消费者需要复用时接入已有产物身份，不另造一层覆盖全部结果的持久镜像。

依据：[Jinja API](https://jinja.palletsprojects.com/en/stable/api/)、[OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching)、[Claude prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)。
