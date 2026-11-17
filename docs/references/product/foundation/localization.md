# 用户呈现与本地化

## 范围

基础管理的帮助、错误和状态支持 zh-CN 与 en。使用标准 gettext 的实例化翻译对象及目录工具；不通过进程全局 `locale.setlocale` 改变并发请求行为。

业务返回稳定错误 code、参数及类型化结果。呈现层选择语言和时区，生成用户文字。日志 event 名和机器字段稳定，用户内容、模型输出和原始文稿不被界面本地化改写。

## 选择顺序

本次命令明确语言 → 实例有效语言 → 默认 zh-CN。缺少对应译文时回退已提供的英文 msgid，并正常工作。时间展示调用时间模块，持久值不受影响。

配置字段有稳定路径和中文描述；面向英文界面可以维护对应翻译 key。首轮只翻译真实暴露的管理操作，不提前生成整套未实现认知 UI 文案。

## 接口与实施

`render_error(error, locale)`、`format_status(result, locale)` 是纯呈现操作。CLI 的 `--json` 返回原类型化合同，仍可带可读 message。模块不要在内部异常中直接拼接多语言大段说明。

译文保存在 `nous.presentation` 的 gettext 目录中，运行时通过包资源加载。修改 `.po` 后执行 `pybabel compile -D nous -d src/nous/presentation/locales` 更新随包分发的 `.mo`。

完成条件：同一错误的 code 在两种语言下相同；并发选择语言不串线；中文路径和用户文本保持原样。来源：[gettext](https://docs.python.org/3.14/library/gettext.html)。
