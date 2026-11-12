# 文稿与提交后维护的使用

初始化新实例后，可以直接保存手写文稿并以 lexical 模式检索。需要语言标注时先配置 `annotate-concepts` 任务和模型网关，模型参数见[配置示例](../examples/models.toml)。

```sh
nous journal create --file ./journal.md --annotate --refresh --refresh-mode lexical
nous read 'jrn(...)@1' --sources
nous query '文稿所讨论的问题' --mode lexical
```

短命令默认等待所请求的维护。正文保存后若标注或刷新失败，输出仍包含正式修订和具体失败；可以对该修订发起新的标注或刷新。

需要后台维护时，先在一个终端启动宿主：

```sh
nous runtime run
```

再由另一个终端保存并查看当前运行期操作：

```sh
nous journal create --file ./journal.md --background --annotate --refresh --refresh-mode lexical
nous runtime operations list
nous runtime operations wait OPERATION_ID
```

后台任务由当前宿主持有。关闭时宿主等待或取消工作并释放实际资源；重开后原操作引用过期，已经保存的正文、成果和快照仍可读取。没有宿主时显式后台请求在保存前返回错误。

SDK 可通过 `async with open_instance(instance)` 在已有事件循环使用完整操作；需要作用域内后台任务时传入 `allow_background=True`。同宿主保存、等待和查询的示例见 [host-use.py](../examples/durable-cognition/host-use.py)。操作诊断通过 `nous runtime logs --operation` 查看。

来源读取使用正式修订及精确选区。Markdown 引用定义、CRLF 与未选择区域随无损正文保留；查询返回的片段可直接按选区回读，并用于记录交付反馈。
