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
nous tools prompt preview summarize --input ./notes.md
nous tools prompt preview summarize --task summarize --input ./notes.md
nous tools prompt edit summarize --from-dir ./my-prompt
nous tools content add ./clip.mp4
nous tools media inspect art:<实际地址>
nous tools media analyze art:<实际地址> --strategy describe_then_structure
nous tools result read res:<实际地址> --sources --details
nous tools embedding run --space chosen-text --purpose document --input ./notes.md
```

纯 preview 不联网；指定 --task 时使用该模型任务的确定性表示。--show-request 明确是显示本次已准备请求并执行。probe 报告实际检验输入与能力，文本样例不宣称验证了声音/视觉。

## 返回、流与错误

普通结果优先正文、短引用、实际覆盖和必要模型说明，内部参数通过 details 展开。content read 的二进制返回元信息和 export 用法。结果 view/provenance 来自共同 owner，CLI 不手写输入关系图展开。

保留当前 CLI 的取消/总超时/部分成功信息。文本流直接交付；JSON 流为 NDJSON delta/completed/failed；completed 必须在正式采用后发出，失败信息保留 partial_output_delivered 和已完成引用。不要将普通 dict 全量挤成不可读的默认产品文案。

独立样本生成和 New API 本地补丁不属于生产 tools，它们继续在自己的开发位置使用。
