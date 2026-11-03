# 音频准备与转写

## 原件与选择

使用 ffprobe 读取真实流、编解码、采样率和时长。必要时用 FFmpeg 转为协议接受的 WAV/MP3，并保留原始音轨与实际时间范围。转换参数属于本次准备结果。

音频长度超过单次能力时按连续时段分批，所有请求范围都得到对应结果或明确的未完成说明。片段的时间说明采用媒体实际坐标，词级或说话人级时间只有上游实际提供时才保存。

## 两种可用输入方式

已配置模型与 SDK 支持 audio content 时，使用原生音频 parts 做理解/描述。

否则使用网关支持的标准音频转写端点，由同一 ModelExecutor 的 transcribe 操作调用官方 SDK，再把实际转写提供给通用文字任务。转写任务、语言提示、输出格式来自配置，不按厂商名做分支。

需要音频的情绪、背景声或非语言内容时，纯转写覆盖不到这些信息。结果明确说明 transcript_only；任务明确要求这些内容时必须选用实际 audio 模型，缺少则报告能力不足。

## 策略衔接

description_only 可以生成听到内容的描述；direct_structured 在准备好的音频或明确转写表示上提取；describe_then_structure 保存描述后继续提取。后两者记录用了原生音频还是转写，不把两者称成相同输入。

转写可以独立成为 ProcessingResult 并保存，即使后续解释失败仍可读取。后续读取该结果不再次提交音频。

依据：[FFprobe](https://www.ffmpeg.org/ffprobe.html)、[Pydantic AI audio](https://pydantic.dev/docs/ai/core-concepts/input/)。
