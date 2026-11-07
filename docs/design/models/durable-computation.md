# 语言任务与成功结果恢复

## 正式结果与任务语义

GenerationOperations 在发送前冻结实际任务的 Prompt 模板、解释合同、模型语义、参数和路线。输入确实需要转写时，冻结对应转写任务；纯文本任务不依赖无关转写配置。冻结内容通过 Artifact 和 ResultStore 保存，凭据仅保留引用。新请求加载冻结任务时核对当前连接绑定与运行限额。

GenerationRequest 包含任务、SelectedInputs、问题、指令、输出 Schema、批次语义和正式执行依赖。稳定 computation_key 标识一次计算；主动重新生成使用新 key。请求身份由语义输入和精确选择决定，显示名称和物理批次不改变身份。

模型协议响应完成并通过输出校验后，GenerationOperations 先采用 model.language_response，再由静态 TaskDefinition 解释有名输出。响应保存实际输入与 SourceContext、原始协议输出、请求参数、实际路线、用量、冻结任务引用以及执行依赖。解释合同保存实际输出 Schema、来源引用及程序产物语义；Prompt 编辑说明和字段帮助不参与判断。

## 恢复顺序

完整 run 或领域 propose 先查询自己的正式结果；已采用的结果直接返回。没有正式产物时，produce 按 computation_key 查找已采用响应；存在则使用保存的实际输入和安装的 TaskDefinition 在本地解释。只有两者都不存在时才准备媒体、核对当前模型绑定并发送新请求。同一计算的并发请求在生成 owner 的短键锁内复用结果，不阻塞无关计算。

recover_response(response_ref, snapshot) 和按原 key 重入 produce 使用同一解释路径。当前 Provider、凭据或 Prompt 是否仍然配置不影响已保存响应的本地解释；安装的解释合同已经改变时返回 model.task_contract_changed，原响应保持可读。

响应已经由远端完成但本地采用失败时，错误保留成功响应及已发布字节；同进程可用 resume_response 完成采用。远端完成与本地首次保存之间仍有进程中断窗口。模型执行器按配置执行有限次数的请求和修复。

## 组合采用

ProducedTask 是进程内的解释结果。prepare_adoption 发布输出字节一次并得到 PreparedProduction；adopt_in(tx, prepared) 在调用者的短事务中采用这些已发布输出，并登记输入、输出映射和执行依赖。事务回滚后可复用同一准备值。ResultStore 的 producer_key 防止同一业务键采用不同结果。

标注生成形成正式提案 ResultRef，之后的标注采用直接读取提案。写作在请求前由 DraftStore 保存 WritingRequest，包含精确 base、来源、反馈映射、写作意图、采用选择与 AfterWrite，并作为语言响应的执行依赖。解释、草稿保存或采用失败返回正式写作请求引用；writing.recover 和 journal draft recover 从该引用继续，保留原基准与意图。已采用草稿重试返回原修订。

执行依赖保存恢复所需引用，不参与独立证据根的计算。字节、实际输入和正式结果分别由 ContentStore、ArtifactStore 和 ResultStore 持有。
