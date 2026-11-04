# CAS 字节与本地借用

## 字节所有权

ContentStore 维护不可变字节、流式读写、物理去重和 owner/slot 引用。SHA-256 是字节身份，不表示认知或来源独立性。采用现有目录和跨卷发布实现，路径通过 ResolvedPaths 提供。

```text
prepare(stream) -> PreparedContent
publish(prepared) -> PublishedContent
attach_in(tx, owner_ref, slot, published) -> BlobRef
detach_in(tx, owner_ref, slot)
resolve_in(tx, owner_ref, slot) -> BlobRef
open(blob_ref, range?) -> async context manager[reader]
read_text(blob_ref, max_bytes) -> str
export(blob_ref, destination) -> Path
borrow_local(blob_ref) -> async context manager[LocalContent]
```

`LocalContent` 暴露只读本地 Path 与 BlobRef，供 PDFium、Pillow、FFmpeg、NumPy/mmap 使用；它只在作用域内有效。ContentStore 负责目录定位、缺失/长度检查和借用生命周期。调用方不通过 `artifacts.content.path_for` 获取永久路径，也不自行推导 digest 目录。

## 发布与事务

流式接纳在 temp 根计算摘要。目标存在时检查实际字节；不存在时使用 content 文件系统内完成态发布。跨卷时复制到 content/staging，再在同卷完成发布。临时文件属于操作，失败后清理。

字节完成后才在短事务中 attach。模型、解码、复制和索引构建不占写事务。已有不可变字节可以复用，数据库回滚不把半份业务结果宣称成功。

## 阻塞与取消

文件、Native 与导出调用共享 Execution 的 blocking 生命周期。Execution.blocking 在调用者取消后等待实际 worker 收尾，资源 owner 随后释放外层句柄；需要返回资源的创建动作优先在同一个 worker 上下文完成取得/使用/关闭，避免取消时丢失刚返回的 fd/path。

CAS 的临时句柄取得必须在第一次可取消 await 之前登记清理：可在事件循环上同步完成短暂的 mkstemp/句柄登记，再把重读写交给 worker；也可把完整资源作用域放进同一个 worker。不要把一个刚创建的 fd 作为会因等待取消而丢失的 worker 返回值。

文件工作与 publish 复用这一公共行为，不再保留第二套局部 shield 生命周期。CAS 仍然决定哪个文件需要清理，Execution 只负责 worker 何时真正结束。初始临时文件创建、写入、flush、发布和取消之间保持同一资源所有者。

## 读取与导出

文本读取受配置上限，超限提供导出或范围读取，不能无标记截断。二进制使用流或 borrow_local，不先加载全部数据才分批。

保留 fdb39e4 的导出行为：仅创建新目标，交付前核对实际长度和 digest；失败清理本次文件，取消等待实际文件操作收尾。普通读取不为每个小操作重做全盘摘要，缺失或长度错误明确返回。

不增加远端多后端框架、后台 GC 或 CAS 版本兼容。将来对象存储有真实需要时由相同 owner 扩展完整操作。
