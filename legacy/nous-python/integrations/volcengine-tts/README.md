# 火山方舟语音合成

通过官方 Agent Plan HTTP 接口生成语音，使用专属上游 API Key。此工具独立于 Nous 模型运行内核。

先按仓库开发说明安装依赖并进入开发环境。将专属 Key 放入本地环境变量，再执行：

```sh
uv run --locked python integrations/volcengine-tts/synthesize.py --api-key-env VOLCENGINE_AGENT_PLAN_API_KEY --text-file temp/narration.txt --output temp/volcengine-tts/narration.mp3
```

也可用 `--api-key-file PATH` 读取本地凭据文件。Key 不作为命令参数传入。默认请求 `seed-tts-2.0`，音色为 Vivi 2.0；`--speaker` 可选择账户支持的音色，`--speech-rate` 调整语速，`--style` 传入语音风格指令。

完整音频与原文、请求参数、实际返回用量分别保存在指定音频路径及同名 `.txt`、`.json` 文件中。产物统一位于仓库 `temp/`。服务未报告的用量保留为空。工具每次只提交一次请求，流中断或错误时移除未完成音频；已有输出需要显式 `--overwrite`。

格式由 `--output` 的 `.mp3`、`.wav`、`.pcm`、`.ogg` 或 `.opus` 后缀推导；显式指定 `--format` 时必须与后缀一致，冲突会在请求前拒绝。例如 `--output temp/voice.wav` 会请求 WAV。

MP3/WAV 默认采用 24 kHz；OGG Opus 需要 `--sample-rate 48000`。PCM 是原始音频数据，使用时须按保存的采样率和服务音频规格解释。`--endpoint`、`--resource` 用于显式选择官方接口与资源，`--trust-env` 允许使用当前传输环境设置。

制作双人对话时，分别为每句指定 `--speaker`，保留逐句原音和口播文字。相同编码与采样率的 MP3 可用 FFmpeg concat 和 `-c copy` 拼接，角色标签留在脚本中。

接口与音色说明：[Agent Plan 语音模型](https://docs.volcengine.com/docs/ark/agent-plan-personal-voice-model?lang=zh)、[HTTP 单向合成](https://docs.volcengine.com/docs/doubao-voice/unidirectional-streaming-text-to-speech-http?lang=zh)、[2.0 音色列表](https://docs.volcengine.com/docs/doubao-voice/tone-list-3?lang=zh)。
