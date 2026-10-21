# 精确时间值

`parse`、`parseRange`、`parseInstant`校验外部输入并返回规范值：`TimePoint {domainId,at}`，`at`为UTC六位微秒。输入必须有明确时区；最多九位小数中的亚微秒部分须为零。合法年份为0001..9999。输出schema只接受规范值。

`readReal()`读取现实时间域`real`。`compare`在同域比较，跨域返回`undefined`。`contains`和`overlaps`接受`{domainId,range}`并保留半开范围：包含起点、排除终点，相邻范围不相交。`range`可缺任一端点，双端点须同域且start早于end。

纯算法迁自Nous原master 412b062082afb75ccfae4c3f69fc606d3e3cc862的`crates/core/src/time.rs`与`packages/client/src/clock.ts`，日期解析和精确比较使用Temporal polyfill。本模块不拥有WorldClock、时间绑定或工作流状态。

`ClockRateSchema`使用非负规范十进制字符串。`micros/formatMicros`在规范点与bigint微秒之间转换；`scaleMicros`精确按倍率向下取整，`addMicros`检查坐标范围。`projectDeadline(snapshot,due)`沿原Client的due/paused/unmapped语义：先判断是否已到期，再处理暂停；未来due按精确倍率向未来取整，跨域返回unmapped。`delaySecondsCeil`在DBOS边界把剩余微秒向未来取整到毫秒后转换为秒。Clock状态和authority仍归各产品。
