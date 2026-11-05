# Journal 维护接口示例

这些文件展示正式接口和可编辑的写作任务资源。程序示例打开已初始化的实例，不启动 CLI，也不调用模型。

- `host-use.py`：一次打开实例，绑定 Namespace，记录、精确编辑、历史读取与词项查询。
- `edit-proposal.json`：专一语言任务返回的小型编辑输出。目标范围来自程序提供的 T 键。
- `journal-edit/`、`journal-rewrite/`、`journal-reflect/`：对应任务的 Prompt 示例，含维护说明和角色消息。

Schema 从对应 TaskDefinition 导出；不再存一份手写完整 Schema。JSON 示例中的 T1 由该次输入准备提供，实际流程使用系统返回的精确引用。通用调用约束见 `docs/development/journal-maintenance-use.md`。
