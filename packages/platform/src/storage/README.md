# 产品存储实例

`createStorage<Database>`组合一个产品的pg Pool、Kysely及官方DBOS datasource。业务表、迁移、CAS和幂等回执由产品owner持有。

```ts
const storage = createStorage<Database>({
  pool: { connectionString, max: 10 },
  datasourceName: 'nous',
  onError: error => logger.error({problem:error.problem}),
});
const result = await storage.migrate(productMigrationProvider);
if (result.error) {
  await storage.close();
  throw result.error;
}
// datasource在DBOS.launch前注册；启动及关闭DBOS由产品组合根负责。
```

普通操作使用`storage.db`；短事务直接`db.transaction().execute(trx => ...)`，同一采用组显式传递同一trx。workflow使用`storage.datasource.runTransaction`与当次`datasource.client`，业务写及检查点共同提交。SDK durableCalls已经处于DBOS step，其内部ModelInvocation事务使用普通Kysely；后续领域采用再进入datasource事务。

认证复用`storage.pool`。LISTEN和Clock authority使用`storage.createSession()`取得同codec、同错误边界的未连接pg Client，自行connect/end，不占业务Pool槽位。storage.close兜底关闭未释放专用会话。正常关闭先停止业务和DBOS，再`await storage.close()`；close幂等，覆盖只经认证Pool使用而Kysely尚未初始化的实例。

连接级解析保留timestamptz的UTC六位微秒、timestamp/date原字符串、int8/numeric十进制字符串及对应数组；JSON嵌套键保持原样。JSON/JSONB中的不安全整数和非有限数拒绝读取；精确大数按正式合同保存为十进制字符串。精确类型使用pg默认text protocol，显式binary格式返回具体问题。

Query/Connect错误在pg Client入口形成安全Problem，丢弃原message/detail/query/cause；`.code`保留pg SQLSTATE以供官方事务重试，公共业务码位于`.problem.code`。Pool后台错误交可选onError，不内置日志。migrate返回官方MigrationResultSet，调用方检查error并决定启动失败的收尾。

复用来源：Nous-Wave原`packages/nous/src/platform/database.ts`的单Pool、CamelCasePlugin和同源Kysely构造方式。官方`@dbos-inc/kysely-datasource`5.3.0及pg/Kysely源码决定实际接线；datasource的checkpoint查询硬编码dbos，因此本实现固定该schema。它与本产品业务/系统表在同一产品数据库。未引入client路由、事务ALS或第二套执行状态。
