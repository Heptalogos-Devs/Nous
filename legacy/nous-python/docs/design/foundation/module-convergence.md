# 模块职责与调用路径

CLI、SDK 和宿主使用同一完整应用操作。Runtime 负责依赖装配与生命周期，领域 owner 通过构造接收实际依赖。

- `application` 定义操作、请求上下文、外部输入和共同正文准备。
- `content` 拥有 Artifact、Result、实际输入、精确来源与映射。
- `models/generation` 执行共同语言调用，任务定义解释各自输出；`media` 拥有确定性表示和媒体分析。
- `cognition/writing`、`annotation`、`feedback` 和 `retrieval` 各自拥有文稿、标注、意见与当前知识视图。
- `cognition/maintenance` 在正文提交后组织可等待或当前进程后台运行的维护。
- `wave` 构建并发布不可变快照，Native 负责检索计算；领域状态由认知 owner 决定。
- `runtime/execution` 跟踪请求与真实资源，`runtime/host` 提供显式宿主及本地传输。

正式采用由完整用例持有短事务，参与者接收已有 `WriteTx`。网络、模型、转换和 Native 构建在事务外执行；内容、成果、草稿、修订与快照分别有自己的保存点。各领域通过类型化引用协作，不按字符串或物理批次猜回来源关系。
