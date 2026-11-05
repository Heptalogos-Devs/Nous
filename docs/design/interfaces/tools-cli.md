# 正式内容与模型工具

`nous tools` 长期提供 model、prompt、media、content、result、embedding 和 [wave](wave-cli.md)。实例、配置、运行诊断和时间命令保持原用法。没有单独 dev 平台。

## 输入与共同操作

CLI 只转换参数、调用完整应用操作和呈现。model run/probe、prompt preview 均使用 GenerationOperations 的共同输入与编译合同；media analyze 使用 MediaAnalysis；embedding 使用 EmbeddingOperations。不得在 CLI 注入临时 tasks、伪造 ConfigSnapshot、创建另一份 Compiler 或通过 runtime.media.preparer 组织语言工作。

同名 --input 按出现顺序保留，--text 在文件/引用输入后按自身顺序追加。完整进程内输入数组可以交错。页码 1 基、--from/--to 秒转换到毫秒的操作只发生一次；其它端点使用同一个 selector 值。

## 保留的操作

```sh
nous tools model list
nous tools model discover gateway
nous tools model probe vision --input ./photo.png
nous tools model run --task summarize --input ./notes.md --show-request
nous tools model run --task summarize --input ./A.md --input ./B.md --question "比较发布日期"
nous tools model run --task describe-media --input ./long-video.mp4 --per-batch
nous tools prompt preview summarize --input ./notes.md
nous tools prompt preview summarize --task summarize --input ./notes.md
nous tools prompt describe summarize --task summarize
nous tools prompt show summarize --origin package
nous tools prompt edit summarize --from-dir ./my-prompt
nous tools prompt import summarize ./my-prompt
nous tools prompt diff summarize
nous tools prompt restore summarize
nous tools content add ./clip.mp4
nous tools media inspect "<完整Artifact标识符>"
nous tools media analyze "<完整Artifact标识符>" --strategy describe_then_structure
nous tools result read "<完整结果标识符>" --sources --details
nous tools embedding run --space chosen-text --purpose document --input ./notes.md
```

纯 preview 不联网；指定 --task 时使用该模型任务的确定性表示。--show-request 明确是显示本次已准备请求并执行。probe 报告实际检验输入与能力，文本样例不宣称验证了声音/视觉。

## 返回、流与错误

model run、media analyze 和 result read 的普通输出显示有名正文与结果短引用；`--details` 展开来源、请求与参数，`--json` 保留完整机器结果。content read 的二进制返回元信息和 export 用法。结果 view/provenance 来自共同 owner。

保留当前 CLI 的取消/总超时/部分成功信息。文本流直接交付；JSON 流为 NDJSON delta/completed/failed；completed 必须在正式采用后发出，失败信息保留 partial_output_delivered 和已完成引用。不要将普通 dict 全量挤成不可读的默认产品文案。

独立样本生成和 New API 本地补丁不属于生产 tools，它们继续在自己的开发位置使用。

## 明确的选项与错误合同

恢复并保留这些产品用法：`--schema PATH` 只用于明确接受自定义 Schema 的任务；`--task` 选择已配置用途；媒体描述任务由 --description-task 或配置选择。`--document-mode auto|visual|text` 中 auto 优先合法原生 PDF、否则按所选页提供视觉/文字；visual 强制视觉页，text 明确只取可提取文字。`--without-audio` 显式排除视频音轨。 模型同时声明 video 能力和视频 MIME 时原生发送视频；--from/--to 对原件时段切片，--without-audio 在发送前剥离音轨。未声明原生视频输入的视觉路线使用采样帧。

参数解析错误也遵守 --json 的结构化错误和非零退出状态；无法归类的异常默认显示清楚的通用失败原因，--debug 展开原始异常链。实际操作错误另带 operation/trace 关联、失败阶段、尝试与部分结果。取消使用现有约定，不输出虚构成功。

model run --show-request 写 stderr 后执行同一请求；纯 preview 只显示并结束。--json --stream 保持 NDJSON delta/completed/failed，completed 在正式采用后发出。帮助文案说明混合 --input/--text 的既定顺序。

model run 与 prompt preview 的 `--per-batch` 显式要求各批独立回答；默认多个输入共同参与一次任务，见 [输入规划](../models/input-planning.md)。Embedding run 的多批结果显示有序完整行和成功引用。诊断用法见 [操作诊断](diagnostics.md)。

## 已有管理与持续读取

Prompt list/show/edit/validate 和 content add/read/export 仍是正式产品能力。未配置模型时也可管理模板与读取已有内容。media analyze 支持显式 --description-result 复用匹配来源及覆盖的描述；复用成功产物不等于重新执行第一阶段。

--show-request 在 stderr 展示实际准备值后执行，stdout 的 JSON/NDJSON 保持机器合同。--pages 的实际页码、--from/--to 的小数秒、文档模式与音轨排除均在帮助中解释；用户不需要复制内部控制 DTO。未来 MCP 按完整用户任务组织工具，不按这些管理命令逐项暴露。

Prompt 的 describe 从真实任务合同和有效配置读取用途、变量、输出 Schema 与模型/协议能力，不发模型请求。edit 在实例副本生成 CONTEXT.md、input-contract.json 和 output-schema.json；三者仅供编辑参考。show 的 --origin 可选 selected、package、instance；diff 比较包内资源与实例覆盖，restore 删除该 Prompt 的实例覆盖并重新选择包内资源。import、edit --from-dir 和 validate 共用同一资源加载与校验。详细合同见 [Prompt 编辑上下文](../models/prompt-editing-context.md)。
