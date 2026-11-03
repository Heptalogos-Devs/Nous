# CAS 字节存储与内容引用

## 责任与输入

ContentStore 保存不可变字节、流式读取、去重和物理回收。采用 SHA-256 内容寻址、本地文件系统、现有 Database/WriteTx 和进程内执行。它不推断 MIME、作者、Observation 或认知版本；[Artifact 与结果](content-records.md)负责可见记录。

输入使用文件流、字节迭代器或已存在 BlobRef。文本明确编码为 UTF-8；原始文件不做换行、Unicode、图片方向或格式的隐式重写。需要转换就产生新字节并保存输入关系。

## 公开能力

```text
prepare(stream, *, cancel) -> PreparedContent
publish(prepared) -> PublishedContent
attach_in(tx, owner_ref, slot, published) -> BlobRef
detach_in(tx, owner_ref, slot) -> None
open(blob_ref, range=None) -> async context manager of stream
read_text(blob_ref, max_bytes) -> str
```

BlobRef 含 digest、byte_length。物理路径由 content 根与摘要推导，调用者不拼路径。PreparedContent/PublishedContent 由当前操作上下文持有；取消时清理本次尚未发布的临时文件。

## 跨磁盘的发布

配置允许 temp 与 content 位于不同磁盘。先在普通 temp 根完成计算后，若无法同文件系统重命名，则流式复制到 **content 根内部的 staging 文件**，复制完成后再在该文件系统内发布到摘要位置。部分复制文件不进入正式摘要目录。

同摘要并发发布在 CAS owner 内按摘要串行，或直接采用已具备的原子 no-clobber 文件操作。只有完整字节才能出现在正式位置；相同长度且完整的既有目标可复用。遇到既有内容长度冲突报告错误，不能静默覆盖。进程只有一个实例写入者，首版无需跨主机发布协议。

发布在 SQL 事务外进行。之后短事务保存正式内容引用；事务失败留下可回收的孤立字节，不能留下指向未完成文件的成功业务记录。

## 引用记录只有一份权威

ContentStore 管理 blob 目录元数据及 owner/slot → blob 的正式关联。Artifact 的正文通过这一关联读取，不再保存第二份可独立修改的 blob 映射。owner_ref 仅由真实记录 owner 提交；本轮只接入 Artifact/ProcessingResult 需要的引用，不建设通用对象状态平台。

ResultStore 在同一事务中提交结果、输出 Artifact 及内容 attach。ContentStore 不读取每个领域私有表来猜引用，也不在读取时全盘扫描。

## 回收与取消

本轮不运行周期性自动 GC。明确回收只在实例维护的独占时段进行：停止新接纳并等待实际文件工作结束，再核对正式引用并清理孤立内容。尚未采用的临时内容由操作直接清理；成功引用的内容不会因请求结束或 Trace 淘汰而丢失。

阻塞文件工作由现有 Execution 管理实际 future；async 等待被取消时，不提前关闭该线程持有的源文件或删除其目标文件。prepare/publish/attach 之间的持有关系留在一次操作内，不建立永久租约服务。

## 读取和交付

二进制下载和本地导出按流完成，范围采用明确字节区间；文本便利读取有配置上限，超过时返回可继续读取的位置或提示使用导出，不能无标记截断。

ModelExecutor 不认识 CAS 路径。应用/媒体准备在字节预算内读取媒体，向成熟 SDK 提供 BinaryContent 等真实数据；外部模型无法读取本机 `file://` 或 `content/...`。

实例重启后按同一 BlobRef 读取；缓存失效只影响性能。Windows、Linux、macOS 的中文路径、空格和跨卷 staging 都由同一实现处理。
