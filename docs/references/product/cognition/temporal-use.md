# 修订、现实变化与待复核关系

## 本轮时间能力

沿用 TimeService 的注册域、Instant/Interval/Unknown 和 occurred/observed/formed/valid/recorded。revision ordinal 与 namespace generation 是提交顺序，不充当虚拟时间。

本轮提供 current、精确 revision、history，以及 Wave 已有时间过滤。发生于未来、后来补录、区间未知均可以保存；不补造时间、不按正文日期猜有效性。任意历史知情 `$asof` 和查询期时间门控仍按 temporal-views 作为后续能力。

## 三种有不同语义的操作

纠错 correction：新的 revision 纠正同一文稿的陈述，旧版保留。实际改变 state_change：旧状态可继续以其适用时间保存，作者选择修订文稿或另写后续 Journal 并建立 successor 引用。后来的 reflection：另成一篇、固定引用当时版本，也可在明确要求下修订原理解。

程序处理明确的 change_kind 和来源，不让模型统一判断所有现实变化。普通 editorial 编辑不自动宣称旧认识有误。

## 显式 current_basis

SourceBinding.current_basis 表达作者明确声明的依赖。`reference` 和 `inspiration` 只用于回看或启发，不触发待复核。

revision correction 可以明确标记被纠正的基准 revision 为 basis_invalidated；普通编辑、archive 和未来新状态不会把旧历史事实整体作废。current_basis 的当前后代通过有索引的引用关系在捕获时计算 needs_review，可沿已有 revision DAG 递归；不逐个写入重复状态、不同步唤起所有领域。

普通 current 检索缺省不把 needs_review 文稿作为可直接依赖的当前认识，结果说明有多少待复核内容；`--include-review` 可阅读并带状态。精确读取与历史仍可用。作者通过新 revision 更新依据，或明确将旧引用改为历史 reference，完成复核。若只是标注/概念变化，不触发文稿真伪判断。

标记依据失效与新修订在同一事务，推进一次 generation。body 原件与旧 source maps 不变。对部分内容更正，第一版作者可以选择按具体 binding/选区声明失效，默认只影响明确使用该 current_basis 的消费者，不泛化成整篇所有引用失效。

## 时间视图的继续方向

严格历史知识视图会选择当时可用的正文、标注、Concept 和关系依据，再构图。不能用今天的几何过滤过去的最终命中来声称还原历史。本轮保存足够的不可变版本和真实 recorded 顺序，暂不创建未使用的时态数据库框架。
