# temp 临时数据与测试材料

`temp/` 保存本地开发工具、缓存、临时工作和测试材料，已被 Git 忽略。长期有用的产品示例、正式脚本和回归测试应放在仓库受版本管理的对应位置。

使用 [temp 管理说明](<ROOT>/scripts/temp/README.md) 和管理脚本建立目录、查看状态及清理：

- Windows：`pwsh -File ./scripts/temp/run.ps1 init`、`status`、`clean`；实际清理使用 `clean -Apply`。
- Linux：`sh scripts/temp/run.sh init`、`status`、`clean`；实际清理使用 `clean --apply`。

命令从仓库根目录运行。清理前关闭正在使用相关目录的任务，将需要保留的材料移入 `test-materials/`；清理默认保留测试材料、工具和虚拟环境。具体清理范围及工具链选项见管理说明。

## 目录用途

- `work/<任务>/`：一次性脚本、探针、实验输出和可丢弃的实例。临时工作按任务集中存放，不散放在 `temp/` 根目录。
- `build/`、`cache/`：可重建的构建产物和依赖缓存；`runtime/`：工具运行时临时文件。
- `reference-code/`：可重新获取的参考仓库和临时源码。
- `tools/`、`venvs/`：本地下载的工具、运行时和虚拟环境；音视频处理可使用其中已有的 FFmpeg 和 ffprobe。
- `test-materials/`：保留的测试输入、实例及可复查结果。需要长期保存的资料放在这里，并保留相关文稿、来源和授权信息。

## 测试材料

文件名以 `0-` 开头的内容由用户提供，优先用于真实材料测试，不得修改；Agent 生成或下载的内容不得使用这个前缀。

`test-materials/` 按用途分类：

- `Text/`：文本和 Markdown 原文，也可以使用项目文档作为测试输入。
- `Image/`：生活场景、文字、插画、像素画、截图和摄影素材；`Probes/` 存放程序生成的探针图。
- `Audio/Speech/`：语音样本；`TTS/` 保存短句、故事、文档朗读和对话，文稿与元信息跟随音频；`FormatChecks/` 保存格式检查样本。
- `Audio/Music/`：有人声和无人声音乐；`sources.json` 保存来源、授权和参考信息，`*-listen.wav` 适合试听。
- `PDF/`、`Video/`：PDF 原件及视频素材。
- `Config/`、`Prompts/`、`Schemas/`、`Collections/`：配置、提示词、结构化输出约束和集合清单。
- `Results/`、`Exports/`、`Diagnostics/`：保留的结果、导出和诊断材料；`Diagnostics/Fixtures/` 包含故意损坏或用于异常场景的素材。
- `Instances/`：需要保留的测试实例；通过 Nous 的 `init` 命令创建。`Incoming/`：清理时从散落目录归集、等待分类的素材。

JSON 元信息中的本地文件引用相对于该 JSON 文件，对话拼接文件中的路径相对于拼接文件。使用素材时保留这些引用关系。

火山方舟 TTS 音频可作为标准音频样本，也可使用仓库中的 [TTS 工具](<ROOT>/integrations/volcengine-tts/synthesize.py)生成新音频。ChatGPT ImageGen 生成的图片可作为标准图片样本；Codex Agent 可通过对应工具或技能生成新素材。

高价值素材包括：

- 用户提供的真实文件内容。
- 正常的 TTS 音频和 AI 图像生成作品；

排除损坏文件、测试夹具及无实际内容的程序探针。
