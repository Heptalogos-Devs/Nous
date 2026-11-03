# 实例装配与可用能力

## 唯一组合根

initialize_instance/open_instance 继续是正式初始化和打开入口。启动前 stderr、路径/锁/资源身份、正式日志、数据库、时间/向量绑定核对及有效配置发布沿用现行顺序。普通打开不创建替代数据库或绕过已登记位置。

组合根显式收集 content、embedding、wave 各 owner 的纯表定义，仅在明确初始化时建表。使用 SQLAlchemy 共同 MetaData 或在组合根汇集为一份可解析外键的 MetaData；同一表只定义一次。内容表不能依赖导入 models.embedding_store 才注册，bootstrap 不再从模型模块借出 content_metadata。纯 schema registry 可以共享，不需要给每张表新建 DI/Repository 层。没有兼容义务的开发格式直接重建授权实例；不自动给未知旧库补表并声称兼容。

## 显式依赖

构造内容与共享 Clock，构造模板/任务目录和模型执行能力；确定性 MediaPreparer 只取内容、执行和配置。GenerationOperations 与 TranscriptionOperations 组织语言阶段，MediaAnalysis 消费它们。ConfigurationOperations 从实际绑定 owner 取得核对能力，不回填已发布服务。

WaveOperations 取得内容、向量、配置、时间、执行、快照存储及 NativeEngine。实际创建 NativeEngine/打开快照可以延迟到首次 Wave 操作；模块导入不启动线程池、不打开数据库或联网。

## 可选能力

缺模型配置时内容读取、纯 Prompt 操作、已存快照的 lexical 和诊断仍可用。Native 扩展不可用时只报告 wave.unavailable；不得在生产中切到一套 Python VCP 备用实现。配置目录的静态 Native schema 不依赖扩展已成功加载。

status 列出真实 ready/configured/unconfigured/unavailable，不隐式构建索引、探测模型或修复资源。完整 Runtime 向入口提供能力，领域构造只接收自己需要的 owner。

## 关闭

先停止新操作，结束已接纳工作、媒体子进程与 Native 实际 worker；再关闭 Native 索引/池、模型连接、内容和数据库；最后观测与日志、实例锁。AsyncExitStack 管理已取得资源，清理失败不覆盖最初原因。

Native 原生 handle 的销毁和 index file/mmap 关闭保持同一所有者；不能在 coroutine 已取消而 Rust 仍运行时提前 close。
