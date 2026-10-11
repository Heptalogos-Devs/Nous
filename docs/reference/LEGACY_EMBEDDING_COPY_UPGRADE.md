[(返回参考索引)](README.md)

# 旧向量归属升级

[返回接口参考](README.md) · [文档目录](../INDEX.md)

向量来源绑定的升级是一次显式、离线操作。Kernel 普通启动不会猜测旧向量的来源，也不会搬动旧数据。请在恢复出来的副本上执行；转换命令不会打开原数据库或原对象目录。

## 准备离线副本

1. 停止会写入源实例的 Kernel。在数据库和对象目录都复制完成前保持停止，确保两者来自同一份一致快照。
2. 将 PostgreSQL 导出为 dump，再恢复到一个新数据库。新库的名称必须与源库不同。将对应的对象存储目录复制到另一个位置。保留原数据库和原目录不变。

```sh
pg_dump --format=custom --dbname="$SOURCE_DATABASE_URL" --file=authority.dump
createdb nous_wave_upgrade_copy
pg_restore --no-owner --dbname="$COPY_DATABASE_URL" authority.dump
cp -a /path/to/source/data/objects /path/to/copy/data/objects
```

按部署的常规机密管理方式提供数据库凭据。避免把含密码的 URL 写入 shell 历史或共享进程列表；需要时使用本机 PostgreSQL 身份验证或受保护的临时 shell。`COPY_DATABASE_URL` 必须连接到恢复出的目标库；其 `current_database()` 值必须不同于 `--source-database-name` 提供的源库名称。

## 转换副本

使用已安装的 `nous-kernel` 二进制，以及现有配置文件中的配置包和数据库连接池限制。转换程序使用显式传入的副本数据库 URL 和副本对象目录，不会使用配置里的源数据库连接或对象目录。它会在目标数据库获取常规 Kernel 排他锁，并使用临时 Serving 目录；不会初始化或调用模型/Embedding provider。

```sh
export NOUS_WAVE_COPIED_OFFLINE_DATABASE_URL="$COPY_DATABASE_URL"
./nous-kernel \
  --config /path/to/source/data/config/apps/nous.toml \
  --upgrade-embedding-ownership-copy \
  --copied-object-root /path/to/copy/data/objects \
  --source-database-name source_database_name
```

也可以用 `--copied-offline-database-url` 直接传入目标 URL；使用受保护的环境变量可避免把完整凭据写入命令历史和进程参数。

命令会报告副本库名称、检查的引用数、恢复归属的向量数，以及无法确认来源但已保留的向量数。如果目标库名称与 `--source-database-name` 相同，命令会拒绝执行。Kernel 普通启动不会自动运行此转换。

转换按 `text_content_identity(representation_text)` 及兼容的 space/producer，将向量绑定到当前或历史上可读文档的精确来源引用。无法归属的旧向量会逐字节保存在独立的 `legacy_embedding_materials_backup` 表中，不再对活动向量缓存可见。程序不会重算向量。请检查升级后的副本，再决定何时替换源实例。

每个 Subject 单独持锁，并以分页方式扫描历史时间点和文档引用；该 Subject 的绑定与未知向量备份在同一事务提交。若转换中断，当前 Subject 的部分结果会回滚，已提交的 Subject 会因不再包含无绑定活动向量而跳过；可在修复原因后对同一副本再次运行命令。

备份表的保留和删除范围独立于活动缓存。清除 Memory、Schema、Episode 或 Journal 来源时，不会删除 `legacy_embedding_materials_backup` 中的行；请单独管理该备份及其删除流程，不要将其与活动缓存归属或普通 owner purge 混为一谈。在确认升级副本可用之前，保留未改动的离线原始副本作为恢复点。
