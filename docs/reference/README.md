# 当前接口参考

[返回文档目录](../INDEX.md)

本目录记录当前 checkout 对外或跨 owner 可观察的接口行为，并提供稳定阅读入口和实现边界摘要。协议源、类型定义、parser/compiler 与实现代码定义具体行为。

- [第一方 CLI Agent Tool](../../apps/nous-cli/README.md)：身份/Tag/关联、准备检查、显式上下文和结构化错误。
- [Official Client operations](CLIENT.md)：公开调用值、完整revision输入、生命周期与consumer权限。
- [NousQL](NOUSQL.md)：TypeScript parser/compiler 的当前语法、绑定行为与限制。
- [Configuration Service](CONFIGURATION.md)：registry、覆盖优先级、快照、digest 和持久化。
- [Model Runtime](MODEL_RUNTIME.md)：标准协议、角色、Prompt、derive/formation 和 producer trace。
- [Capability Composition](CAPABILITIES.md)：Process/Subject capability 与 Memory-only 组合。

长期语义和目标 ontology 见 [Architecture-Vault TARGET_DESIGN.md](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/docs/Nous-Wave/TARGET_DESIGN.md)；当前产品行为与 owner 合同见 [Specs](../specs/INDEX.md)。
