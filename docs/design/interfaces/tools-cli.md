# 正式模型工具与开发辅助命令

## 命令归属

`nous tools` 是正式产品命令前缀，用于直接操作模型、Prompt、媒体、内容和结果。它长期保留，供人和其他程序使用，后续认知命令可以复用同一内核。

现有 init、instance、config、runtime、time 是实例管理入口，保留原语义。本轮不建立 `nous dev` 命令框架。临时样本生成、故障实验和单次诊断脚本放忽略的 temp；需要长期保护的高价值情境进入现有 pytest 的通路测试，不进入产品工具目录。

## 正式命令

所有例子都继承已有 `--instance`、`--json` 和显示语言选项。

```sh
nous tools model list
nous tools model discover gateway
nous tools model probe vision --input ./photo.png
nous tools model run --task summarize --input ./notes.md
nous tools model run --task summarize --input art:<短地址> --show-request
nous tools model run --task summarize --input ./notes.md --stream

nous tools prompt list
nous tools prompt show summarize
nous tools prompt edit summarize
nous tools prompt validate summarize
nous tools prompt preview summarize --input ./notes.md

nous tools content add ./photo.png
nous tools content read art:<短地址>
nous tools content export art:<短地址> --output ./photo-copy.png
nous tools media inspect art:<短地址>
nous tools media analyze art:<短地址> --strategy description_only
nous tools media analyze ./photo.png --strategy direct_structured
nous tools media analyze ./paper.pdf --pages 2-4 --strategy describe_then_structure
nous tools media analyze ./clip.mp4 --from 0 --to 12 --strategy description_only
nous tools media analyze ./photo.png --strategy describe_then_structure --description-result res:<短地址>
nous tools result read res:<短地址> --sources

nous tools embedding probe text-embedding --input ./notes.md
nous tools embedding run --space personal-text --purpose document --input ./notes.md
```

占位短地址必须使用实际返回值。`--from/--to` 在 CLI 使用秒，可包含小数，内部统一换为媒体毫秒；`--pages` 1 基包含端点。`--input` 可重复并保持同名参数的出现顺序；`--text` 可重复提交文字，按出现顺序追加在文件/引用输入之后。帮助和预览明确此规则，不假定不同选项交错顺序会由 CLI 框架自动保留。进程内 GenerationRequest 的 inputs 本身是完整有序数组。

`probe` 报告本次实际检查的能力；未提供媒体样例时只验证可执行的文字路径，不以一次文字响应认定视觉/音频可用。`--schema PATH` 仅用于 custom-reading 合同，使用该任务明确的 native/tool 输出模式。

`--task` 选择已配置用途。media 默认任务见媒体配置，用户也可指定最终 `--task`；两阶段的描述任务独立由配置或显式 `--description-task` 指定。PDF 可用 `--document-mode auto|visual|text`：默认 auto 优先采用当前协议支持的原生 PDF，否则保留所选页图与文字；visual 强制页图方式；text 明确只取可提取文字。视频的 `--without-audio` 明确排除音轨。说明这些选项的具体作用，不能向用户泄露内部阶段控制 DTO。

## 常用返回

文本显示正文、结果地址、输入来源和必要模型身份。内容 read 遇到二进制时显示元信息与导出用法；不向终端吐二进制/base64。`result read --sources` 展开实际原件、页/时段、描述和输入合同；高级参数可用 `--details` 展开。

JSON 返回公开类型化结果，正文保持可用。stderr 承担进度与日志。普通引用不要求用户复制 UUID、SHA-256 或长配置 JSON。

## 流与持久结果

`--stream` 首轮用于文字 generation。人类模式实时写文本，结束后给出最终结果引用；中断明确提示部分输出。`--json --stream` 使用 NDJSON：delta、completed 或 failed 事件，completed 必须在正式结果提交后发出。

结构化任务默认完整返回，media 多阶段默认显示进度并返回完整或明确部分结果。本轮不把流内容误保存成成功终稿，不在中途 fallback 拼接。

## 预览与编辑

`model run --show-request` 在同一进程将准备结果显示到 stderr 后执行，无默认审批停顿；单独 `prompt preview` 只显示并结束。--show-request 不改变 stdout 的 JSON/流式协议。无模型配置时仍可进行纯模板操作。

`prompt edit --from-dir PATH` 可导入可信的完整资源目录，普通 edit 使用编辑器。输出 Schema 由任务合同或明确的自定义 Schema 管理；编辑任务文字不会无声改变应用输出类型。

## 适配器边界

命令函数仅转换参数和呈现。业务进入 ConfigurationOperations、GenerationOperations、MediaAnalysis、ArtifactStore/ResultStore、EmbeddingOperations。离线和在线配置用同一操作合同，CLI 不再内联开启数据库核对绑定。

正式 tools 数量不决定未来 MCP 的工具数量。MCP 以后围绕查询/读取/处理等真实使用设计，不把所有管理命令逐项搬成模型工具。
