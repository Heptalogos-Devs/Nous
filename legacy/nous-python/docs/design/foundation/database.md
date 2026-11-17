# 数据库连接与实例存储

## 选择与范围

使用 SQLite 本地文件、SQLAlchemy 2 Core 和 aiosqlite。首轮完成数据库打开、基础 schema、读写事务和关闭。数据文件来自已解析的 data 根，不要求模型或 Trace 可用。

一个实例默认一个应用写入进程。认知、内容和检索表由各领域拥有，基础层不定义通用 Cognition JSON 大表。

## 连接与事务模式

使用 SQLAlchemy `create_async_engine` 和 aiosqlite。连接工厂在事务开始前设置 PRAGMA，由数据库层显式开启读事务和 `BEGIN IMMEDIATE` 写事务。

数据库初始化时，在独立的无事务设置连接上开启 WAL；每个连接确保外键启用及相同 busy_timeout。PRAGMA 的设置必须在它要求的事务外完成，不能在已自动 BEGIN 后假定生效。SQLite synchronous 默认 FULL；对 NORMAL 的选择是明确高级修改。

数据库文件、WAL 和 SHM 的生命周期交给 SQLite。正常关闭 dispose 引擎，不手工删除运行中的附属文件。SQLite 的 WAL 用于本地文件系统；跨主机共享磁盘不是当前部署合同。

正常 open 使用现有文件模式，文件不存在就返回 storage.missing；只有明确 init 创建数据库并建表。不能因为操作者改了路径，就在新位置自动建一个空库并报告正常。通过 SQLite URI 的 mode=rw 等成熟方式落实，不只依赖一次可能发生竞争的 exists 检查。Windows 文件路径通过 SQLAlchemy URL/路径能力构造，避免手工拼接冒号和反斜线。

## 基础表

`instance_metadata` 保存 instance_id、独立的 database_id 和当前 schema 格式。正常实例打开把配置目录绑定的 database_id 传给数据库核验；相同 instance_id 的另一个独立数据库会被拒绝。`time_domain_definitions` 保存 domain_id、kind、tick_seconds 的分子分母、epoch 描述及显示信息。其结构按时间专题实现。

各内容与认知 owner 提供自己的正式表；运行期 Task 不写入任务表。

开发数据没有旧内部 schema 兼容要求。不兼容格式返回清楚错误，按当前授权重新初始化指定开发实例。不得为了本次重构扫描或清空其他实例、未知目录或未授权用户文件。

## 公开内部能力

```text
Database.open(path, settings, instance_id, *, create=False, database_id=None) -> async context manager
Database.read() -> async read transaction
Database.write() -> async WriteTx
Database.check() -> DatabaseStatus
```

ReadTx 和 WriteTx 向参与者提供 SQLAlchemy Core connection。领域不取走引擎去自行创建第二套连接策略。一次读取需要一致性时在同一 read transaction 内完成。

应用内写入序列化锁只覆盖短事务。SQLite 写锁与 busy_timeout 负责协调其他连接。

失败回滚当前事务。锁冲突或快照冲突返回相应错误，由完整业务操作决定是否重新读取再尝试；数据库模块不重放任意包含模型或文件副作用的闭包。

## 生命周期

Bootstrap 在发布运行时前验证文件和实例身份。错误时保留数据库原状并释放连接。关闭前结束使用它的操作与工作执行，随后 dispose。

## 完成条件

init 与重新打开保留同一实例；实际写入、读取及异常回滚成立；外键约束实际启用；事务外长任务不持有写锁；数据库无法打开仍能通过 stderr 获取原因。不执行库级断电、文件系统故障风暴或分布式容灾研究。

依据：[SQLAlchemy SQLite dialect](https://docs.sqlalchemy.org/en/20/dialects/sqlite.html)。
