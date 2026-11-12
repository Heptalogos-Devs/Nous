# 多轴时间、时间域与运行时钟

## 目标

统一表达材料入站、接触、形成、适用、修订记录的时间，同时支持模拟或外部时间域。执行超时和持续时长使用独立运行时钟。

首轮实现时间值、域注册、解析、比较和格式化，以及数据库记录时间；不实现世界模拟、日历、自动推进或认知查询器。

## 三种时钟责任

`Clock.utc_now()` 返回带 UTC 时区的真实记录时刻。`Clock.monotonic_ns()` 用于耗时、剩余预算及截止期限。`DomainClockSnapshot` 保存调用方提供的域内当前坐标，以支持将来的“最近三天”等相对认知查询。

单调时钟用于当前调用的期限；业务日期和时间域条件使用各自明确的时间值。

内置真实时间域 `utc` 使用 Unix epoch 微秒。序列化的对外 UTC 时刻使用带 `Z` 或明确 offset 的 ISO 8601；禁止用无时区 datetime 偷偷假定本机时区。系统时钟可能校正，因此数据库提交顺序使用独立递增序号，不靠 recorded_at 全局排序保证。

## 五轴及各自所有者

| 轴 | 事实 | 谁提供 |
|---|---|---|
| occurred | 原事件发生时间 | 原事件来源；可以未知或处于非现实域 |
| observed | 某个观察者实际接触的时间 | 接触操作或可信宿主声明 |
| formed | 某项内容或认识实际形成的时间 | 实际形成/采用操作，保留来源声明 |
| valid | 某项内容适用的时间范围 | 对应认识或状态 owner |
| recorded | Nous 实际保存该正式记录的时间 | 数据库写入操作，真实 UTC |

材料 `received_at` 是材料接纳操作的真实时刻；不能拿它补造 occurred 或 observed。`modified_at` 可以是最新修订的 recorded_at 视图，无需另造第六种业务轴。一次重复接触有自己的 observed，修订有自己的 formed/recorded。

不用每行强填五个字段。事件时间属于事件，接触时间属于 Observation，认知形成和适用范围属于相应版本。查询沿明确来源关系取值；不同时间约束必须作用于同一相关事实组，不能分别从两个不相干来源拼成命中。

## 时间域

`TimeDomainDefinition` 包含稳定 domain_id、kind、name，以及 linear 域的正有理数 tick_seconds 和可选 epoch 说明。kind 为 `utc / linear / opaque`。utc 是内置不可变定义。

`TimePoint` 是 domain_id 与有符号整数 ticks。只有同一域中的点可以直接比较。两个域都使用秒单位，仍然需要明确映射才能换算；相同单位没有提供共同起点。

linear 的 tick_seconds 描述坐标单位，不描述世界推进速率。加速、暂停、恢复或历史回填由提供该域的系统负责。调用方可以直接提交 domain 当前点；Nous 不用真实时钟猜测虚拟现在。

首轮不实现跨域动态映射。跨域比较返回 `time.domain_mismatch`；缺少相对时间所需当前点返回 `time.clock_required`；opaque 域只接受坐标比较，拒绝将 `30d` 自动换算。

domain_id 在实例域目录内唯一；多个外部世界使用不同域 ID，各 Namespace 引用对应定义。跨实例迁移时随数据携带域定义并核对同名冲突，不按显示名称认定相同。

域定义在首次登记后保持坐标解释不变。名称可以改变；kind、tick_seconds、epoch 改变需使用新 domain_id。数据库保存旧定义，使旧数据仍可解释。

## 时间值与精度

采用明确的可判别值：

```text
Unknown(source_text?)
Instant(at: TimePoint)
Interval(start: Boundary, end: Boundary,
         meaning: duration | uncertain_instant,
         precision?, source_text?)
Boundary = Known(TimePoint) | Unbounded | UnknownBoundary
```

已知区间采用左闭右开。`Unbounded` 明确没有该侧边界，例如有效状态从某天持续且未设结束。`UnknownBoundary` 表示不知道边界，不当作无限远。结束点缺失与尚未结束分别编码。

“2026-11-03 某时发生”解析为该日期在指定时区中的一天范围，meaning=uncertain_instant，precision=day；不能伪装成午夜整发生。某状态全天有效则 meaning=duration。原始“昨晚”“约三天前”等文本可保留，只有上下文充分时解析。

不解析自然语言的普通正则推断。确定的 ISO、日期、整数坐标和声明时区由程序解析；自然语言解释由相应语义任务提供并保留原表达。

## 共同操作

```text
parse_explicit_time(value, domain, timezone?) -> TemporalValue
compare_points(a, b) -> less | equal | greater
match_range(value, query_range) -> matches | no_match | unknown
format_time(value, language, timezone?) -> str
register_domain(definition, tx) -> TimeDomainDefinition
```

完全已知的 duration 按区间相交判断。已知边界已经排除交集则返回 no_match；未知边界导致无法确定时返回 unknown。unbounded 按数学无限边界处理。

uncertain_instant 的可能范围被查询范围完全覆盖时返回 matches，完全不交时 no_match，部分交叠时 unknown。这样“只知道当天发生”不会精确命中该天任意一小时。

不知道时间不意味着时间为零或现在。调用者可以显式选择保留 unknown 结果；默认精确时间过滤只接受 matches，并说明未定范围。排序将未知时间置后；跨域没有自然全序时保持分组或报告无法比较。

## 历史截面

recorded 截面回答系统在真实某时已经记录什么；observed/formed 与所在域回答某观察者或主体当时已知什么。发生得早但后来补录的事件，在旧 recorded 截面不可见。事后形成的新解释也不能进入更早的知情截面。

未来数据库历史读取需要同时保存精确版本与提交序号。首轮只建立共同值与提交时间责任，不预先建设全部认知历史表。

## 跨平台

使用标准 datetime、zoneinfo 和第一方 tzdata 依赖，保证 Windows 也有 IANA 时区数据。DST 重复本地时间需要 offset 或明确 fold；不存在的本地时间返回输入错误，不能静默平移。展示时区改变不修改持久坐标。

## 关键验证

覆盖：非现实 linear/opaque 域、发生与补录不同时间、未知与开放终点、一天精度与一小时查询、记录时间不替代接触、跨域拒绝比较、UTC 和 DST 解析、系统墙钟变化不影响超时。不要测试语言库自身的全部日期运算。

来源：[Python time](https://docs.python.org/3.14/library/time.html)、[datetime](https://docs.python.org/3.14/library/datetime.html)、[zoneinfo](https://docs.python.org/3.14/library/zoneinfo.html)。
