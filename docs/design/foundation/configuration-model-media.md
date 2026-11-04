# 本轮模型与媒体配置字段

## 定义原则

字段在负责模块使用纯 Pydantic 类型、约束和说明定义，统一目录导出。下表给出本轮需要落实的字段；示例值是起步默认，不宣称最优。未定义高级参数不硬编码到调用点，也不为尚无用途的库内部参数逐项造配置。

现有 Instance、Paths、日志、执行和 SQLite 配置继续生效。新增字段的元信息须含 description、exposure、scope、apply_mode、单位与改变后果；模型的动态资源子字段也要有可读说明。

## 内容与媒体

| 字段 | 类型与默认 | 生效/用途 |
|---|---|---|
| content.io_chunk_bytes | int=1048576，>=4096 | restart；流式接纳块大小 |
| content.text_read_max_bytes | int=8388608，>0 | next_operation；便利文字读取上限 |
| media.ffmpeg_path / ffprobe_path | nullable path=null | restart；null 用 shutil.which，找到后记录实际版本 |
| media.max_inline_bytes | int=33554432，>0 | next_operation；一次模型请求内物化媒体的技术上限 |
| media.image.max_side | int=2048，>0 | next_operation；需要规范化时长边上限 |
| media.pdf.render_dpi | int=144，>0 | next_operation；页面视觉表示 |
| media.pdf.max_pages_per_request | int=12，>0 | next_operation；分批大小，不能据此截断整份文档 |
| media.video.sample_interval_ms | int=2000，>0 | next_operation；通用抽帧目标间隔 |
| media.video.max_frames_per_request | int=12，>0 | next_operation；分批大小 |
| media.audio.max_seconds_per_request | number=60，>0 | next_operation；音频分片大小 |
| media.audio.transcription_task | nullable ResourceId=null | next_operation；需要标准转写端点时选用的任务 |
| media.analysis.description_task | ResourceId=`describe-media` | next_operation；两阶段描述任务 |
| media.analysis.extraction_task | ResourceId=`extract-information` | next_operation；结构化任务 |

输出图像格式、音频采样率/格式等按目标协议支持进行明确选择，实际转换参数写入结果；用户需要调整的项由各媒体配置模型提供。默认不启用自动 GC 或后台维护定时器。

## 模型与任务补充

Provider 的 protocol/endpoint/credential_ref 以及 network 中的代理、CA、trust_env、连接池采用 restart；各请求 timeout 为 next_operation。Model 添加 `input_formats`（MIME 列表）、`output_modes`（text/native/tool）、可选协议 compatibility 设置，均在下一操作准备请求时采用。它们描述接入合同，不按 model_name 的字符串分支推断。

TaskAssignment 添加 `operation=generation|transcription`（默认 generation）；generation 要求 prompt，transcription 允许无 prompt，若有则只允许单个提示文本，不能伪装 system/user 多角色能力。`output_mode=text|native|tool` 为 generation 输出选择，默认 text；实际可用模式须与模型声明/协议一致。

任务名称是可配置的 ResourceId。Prompt 的 contract_id 指向任务 owner 提供的输入/输出值类型。通用用户结构化任务可指定 JSON Schema 文件，由标准 jsonschema 校验，详见 Prompt 专题。

fallback、媒体策略和模型选择沿用已定义参数。三种媒体解释策略只在需要解释时生效，media inspect 保持确定性。

## 费用与技术限制

本轮 `max_cost=null`，不建设充值、余额查询、费用批准、预扣或预算账本。真实用量照常记录；缺少价格时金额未知不妨碍调用。

输出 token、上下文、内联字节、期限与有限重试是实际协议/运行参数。分批及转换围绕这些限制工作，不能静默丢输入。多阶段调用的次数语义见 [额度](../models/budgets.md)，不把两次正常语言任务都当作同一次请求的重试。

## 本轮新增字段的归属

媒体输出格式偏好在 image 配置中定义，供原图、PDF 页图和视频帧共同消费；默认依次为 PNG、WebP、JPEG，并与当前模型接受格式取有意义的可用选择。JPEG 背景合成值缺省 null，只有透明图必须转换 JPEG 时要求显式 RGB 背景。编码质量沿用现有参数，缺少时在同一配置定义中补齐，不散在转换函数。

批次预取缺省 0，表示按需顺序转换；本轮可保持此值，不为尚无实证的吞吐问题增加预取调度。Embedding 继续使用现有 max_batch_size，所有入口由共同执行者消费。

诊断配置增加终端 basic/verbose（缺省 basic），结构化文件保留完整关联。Span 事件容量采用 SDK 的有界默认并允许在 tracing 配置中覆盖，不固定禁用全部事件。这些字段具有类型、描述、范围和生效方式，实际消费者接通后才报告可用。
