# New API 音频转写适配

本补丁基于官方 **v1.0.0-rc.47**（`e835b1bfc43b565d625537830dbfb19328925134`），为高级自定义渠道增加音频转写到 Chat 的协议转换。它面向接受标准音频内容的 OpenAI Chat 兼容上游；调用方使用 `/v1/audio/transcriptions`。

## 启用范围

功能通过已有 Advanced Custom 路由合同显式启用。管理员在该渠道原有 `advanced_routes` 数组中追加以下对象，并按真实上游填写目标路径和支持音频的模型：

```json
{
  "incoming_path": "/v1/audio/transcriptions",
  "upstream_path": "/v1/chat/completions",
  "converter": "openai_audio_transcriptions_to_openai_chat_completions",
  "models": ["audio-capable-model"]
}
```

该路由参与现有的模型、分组、优先级、权重和渠道固定规则；它的鉴权与网络设置也沿用该渠道。管理员通过现有优先级配置选择原生 ASR 渠道或转换渠道。其它转换器和原生转写通路保持各自的协议处理。

构建补丁不会改写任何渠道、路由、模型映射或凭据。已有配置不会自动启用转换；部署方需要显式添加路由。独立验证可以使用临时实例中的等价配置，保留现有实例原样。

## 转写合同

该后端让音频理解模型执行**原语言逐字转录**：保留重复与口误，不翻译、不总结、不描述环境音。`language` 是源语言提示，`prompt` 是词汇拼写提示。返回文字来自实际音频输入的模型输出。

这条通路使用音频 Chat 模型，识别质量由所选模型决定。它不调用独立 ASR 服务，也不产生 ASR 的词级置信度、说话人分离或时间戳。

支持一次一个 WAV/MP3 文件、`language`、`prompt`、`temperature`、`response_format=json|text`，以及 JSON 格式的 `stream=true`。时间戳、`verbose_json`、SRT、VTT、说话人参数和其它未支持字段会在提交上游前报错。音频不被截段或静默缩短；超出模型能力时由调用方分批。

JSON 响应提供 `text`、返回的 `model` 和 `transcription_backend=audio_chat`，上游报告用量时同时提供标准转写格式的 `usage`。上游未报告用量时省略该字段；本地估计只用于账本，并在日志中标明来源。文本响应的正文只有转录文字。响应头 `X-Transcription-Backend: audio-chat` 标明实际后端。上游用量与请求转换关系沿现有计费和日志通路保存。

音频 Chat 使用其真实输入、输出及音频 token 结算；原生 ASR 延续其既有 token 或时长计量。两种计价语义应使用独立的模型别名与定价表达式，同一请求的跨渠道重试会拒绝切换计量语义。token 计价的流即使随后中断，已知消耗仍会结算；按次计价的失败保持原有退款规则。

流响应采用 `transcript.text.delta` 和 `transcript.text.done` 事件。正常结束后才发送 done；截断、过滤、上游错误和取消均不会把半截文字标成完整转写。取消传递给当前上游 HTTP 请求，流体由现有扫描器关闭。

## 构建

需要 Python 3.12+、Git 和 Docker。在仓库根目录执行：

```console
python integrations/new-api/build.py
```

脚本每次在 `temp/new-api-audio-build/source-<random>` 创建独立源码目录，获取固定官方版本并检查提交及初始工作区，应用 `patches/0001-audio-transcription-chat.patch`。随后按照上游发行流程把固定 tag 写入 `VERSION`，用上游 Dockerfile 构建完整镜像（含原管理界面），使界面和二进制均报告 `v1.0.0-rc.47`。已有目录及其改动保留。保留上游 Go 1.26.10 安全修复。镜像名默认是 `nous-new-api:rc47-audio-transcription`。`--prepare-only` 只准备可检查的源码，`--image` 指定输出镜像名。

此脚本只构建镜像，不启动、停止或更新容器，不读取或注入渠道凭据，也不迁移数据库。独立环境验证及后续采用由部署方完成。

普通构建在成功、失败或取消后清理本次创建的源码目录；`--prepare-only` 保留源码供查看。清理前核对目录仍位于仓库 `temp/` 内且身份未变化，预先存在的目录和用户改动不会被删除。

上游的 AGPL-3.0、版权和第三方声明随上游源码及镜像保留。本目录的补丁覆盖所修改与新增的完整源码；分发修改后的服务时应一并提供对应版本、补丁和构建方式。

## 官方依据

- [rc.47 发行版](https://github.com/QuantumNous/new-api/releases/tag/v1.0.0-rc.47)
- [标准音频转写接口](https://docs.newapi.pro/zh/docs/api/ai-model/audio/openai/createtranscription)
- [高级自定义适配器](https://github.com/QuantumNous/new-api/blob/v1.0.0-rc.47/relay/channel/advancedcustom/adaptor.go)
- [渠道路径过滤](https://github.com/QuantumNous/new-api/blob/v1.0.0-rc.47/model/channel_constraint.go)
- [OpenAI 转写及流事件合同](https://developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create)
