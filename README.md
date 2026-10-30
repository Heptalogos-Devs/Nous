# Nous

Nous 是独立安装的长期认知服务，为智能体提供记忆、知识、来源、时间、检索与认知成长，也是 Heptalogos 的认知和人格中心。Python 主应用负责领域、模型、授权、事务与 CLI、HTTP、MCP、Python SDK；必要的 Native 计算承担 VCP、NousQL 等性能热点。

产品目标以[项目宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)为准。Heptalogos 基于 MaiBot Fork，负责主体的生活、世界、注意和行动；双方通过稳定逻辑主体身份与正式接口协作，各自持有数据。Nous 的领域保持独立模型，A-Memorix 公共接口通过边界适配器接入同一认知权威。

## 领域

| 名称 | 职责 |
|---|---|
| Nous Memory | 记忆、Episode、Journal、来源与认知修订 |
| Nous Wave | 词项、向量、图关联、VCP、候选融合与重排序等检索算法 |
| Nous Ego | 自我认识、人格倾向、种子、自我叙事、知识与能力边界 |
| Nous Social | 人物、群体、社会关系、跨平台身份认识与社会交流惯例 |
| Nous Context | 当前认知、工作上下文、认知投影与模型实际消费 |
| Nous Motivation | 愿望、关切和期望状态等长期认知 |

NousQL 是整个 Nous 的查询语言，由 Wave 执行检索。Ego 持有主体对自己的认识；当前心情和实际行为选择由 Heptalogos 持有。Social 的设计对黑话、表情包、GIF、音频梗等社会符号共同组织载体、内容描述、表达含义、情绪或意图、对象、情境和社会使用经验。

## 当前状态

当前 `main` 以 Python/Native 实现为基础。模型多路线改造尚未完成应用组装：`ModelService` 需要 `RoleRegistry`，当前源码尚不能直接启动。已有运行结果使用此前加载的代码；实际记录和待完成事项见[重构进展](docs/python-native-plan/progress-report.md)。下一步完成角色注册与应用接线，并继续统一修订状态、A-Memorix 完整操作及真实 MaiBot 消费路径。

仓库保留独立工作台源码；主要管理入口为 API 与 CLI，工作台的后续去留依据真实用户需要决定。开发目标见[GOAL](docs/GOAL.md)，技术组织见 [Python/Native 设计](docs/python-native-plan/README.md)，安装与开发命令见[安装说明](docs/python-native-install.md)。上述启动缺口修复后，按实际版本使用这些说明。

## 产品范围

Nous 支持材料、来源及五轴时间，Memory、Episode、Journal、Ego、Social、LanguageConvention、Motivation 和知识检索。A-Memorix 已有的检索、画像、Episode、导入、图谱、维护、纠错、Bundle、图片及管理接口是需要完整覆盖的功能基线；已接入和未完成的具体范围见[进展](docs/python-native-plan/progress-report.md)。CLI、HTTP、MCP 和 Python SDK 是正式调用入口。
