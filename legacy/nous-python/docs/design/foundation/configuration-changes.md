# 配置修改、生效与正式绑定

## 修改效果

live 用于已明确支持的轻量即时动作，当前为日志级别。next_operation 只影响后续操作。restart 保存期望设置，运行中资源继续原快照，重开整体采用。new_binding 的修改需要真实 owner 核对持久数据意义。

作用范围、呈现层级和改变影响独立。固定实例身份不通过普通 set 修改；正式数据根移动使用明确重定位；时间域和向量空间由各 owner 判断。

## 唯一修改用例

在线与离线入口共同使用 [ConfigurationOperations](configuration-integration.md)。CLI 不内联绑定规则；bootstrap 只装配，不向已构造服务补 callback。公开 update/unset/remove-resource/reload 保持同样的输入语义和结果。

update 可带 expected_version，冲突返回实际期望版本供调用者决定。普通修改无强制 preview、dry-run 或双重确认。预览是使用者主动选择的操作。

## 过程与结果

候选解析/校验及只读绑定检查通过后原子保存文件。无需重启时，再采用正式绑定和 live 设置，全部成功才发布新有效快照。日志轻量应用失败时恢复原级别；数据库绑定提交不伪装成文件事务的一部分。

结果包括保存位置、变化字段、保存值、当前有效值、来源、是否被高优先级覆盖、期望版本和有效版本，以及 saved/applied/saved_pending_restart/rejected。离线保存只报告 saved。

含 restart 字段的修改整体等待重启。已经 pending 的后续修改也只保存期望配置，不局部混入当前资源。下一次成功打开整体采用并清除等待状态。

## 失败语义

| 位置 | 实际状态与返回 |
|---|---|
| 校验或只读绑定检查 | update 未写入；reload 保留用户已经编辑的文件；有效快照不变 |
| 临时写入/原子替换 | 原文件与代保持，清理临时文件；configuration.save_failed |
| 文件已保存，绑定采用失败 | 文件与期望代前进，有效快照不变，saved_pending_restart；configuration.apply_failed |
| 绑定已提交，live 应用失败 | 明确绑定已采用、日志/有效快照仍旧，等待重启，不删除已提交定义 |

详情给出 file_saved、write_performed、version/effective_version、phase、已知绑定结果和 recovery。数据库提交中遇到取消或驱动失败时，可标记 binding_application=unconfirmed，重开从正式表核实。取消保留取消语义，状态可从配置读取。

保存后采用失败时，后续修改继续待重启；不增加一套自动恢复/补偿框架。正常错误用同一公开合同交付。

## 时间域与实例

相同时间域 ID 的 kind、tick_seconds、epoch 不改变；显示名可修改，新解释用新 ID。opaque 域不接受尺度，纯配置校验与正式域定义一致。

实例绑定核对覆盖文件编辑、环境和启动参数。data/content/index 或数据库位置改变需明确重定位，正常 open 不创建空替代资源。保留首轮已完成的配置目录与数据根占有、资源 UUID 和 portable 整体移动规则。

## Embedding

本轮接入真实 EmbeddingSpaceRegistry，取代首轮全拒绝分支。新、未用的空间声明可编辑；首次正式向量采用同时登记合同。已使用空间的语义变化、相关模型真实身份变动由 owner 拒绝；改用新 ID。

不能仅比较模型资源名。解析后的模型/部署、维度、模态、输入编码和归一化参与判断，实际规则见 [Embedding](../models/embeddings.md)。删除配置不会删除已用空间的正式定义和向量历史。

密钥和显示字段不塞进模型输入。没有通用 force 开关可以跳过正式绑定。金额默认不限制，不添加费用审批状态。
