# 常用实例选择与持久宿主

## 日常命令

保留现有显式 --instance 路径、NOUS_INSTANCE 和平台默认实例。使用轻量 profile 注册，免去每次复制长路径：

```text
nous instance register local D:\Nous\my-instance
nous instance use local
nous instance current
```

register 只保存“名字→现有实例配置目录”，use 保存所选名字；不隐式初始化数据库、不复制模型配置。`--profile <name>` 可显式选择某个已登记项；与 --instance 同时出现报参数冲突。

## 唯一解析位置

InstanceLocator 统一解析：显式 path/profile → NOUS_INSTANCE → 用户选择的 profile → 平台默认实例。明确的 profile 缺失或路径无效应报具体错误，不悄悄落回另一实例。

注册信息存 platformdirs 取得的 Nous 用户配置目录中的 client.toml 的 `[instances]` 表；该文件中的其他客户端设置保持原样。它不是实例业务配置或凭据库。Namespace 默认值继续归实例 cognition 配置，不在 client 文件再复制一套默认域。`InstanceProfiles` 管理注册与选择，和实例配置共同使用 `common.files.write_text` 原子发布 TOML。

实例 UUID 继续从该实例配置与资源绑定读取，普通使用无须输入。程序可显式传路径来固定宿主实例；工作区的 Namespace 句柄绑定稳定身份，不因为后来另一个 CLI 修改默认 profile 而换实例或域。

## 生命周期

open_instance 仍持有唯一运行期资源锁。一次长程序持续使用同一 Runtime，不每次业务调用重新启动模型连接、数据库和 Native。profile 选择不改变锁模型，也不启动后台守护进程。

常驻宿主通过本地认证通道提供同一应用操作；实例不自动启动后台服务。

`instance current` 返回本次实际选中的 profile、配置目录和 nous.toml；显式路径与 NOUS_INSTANCE 选择保留 profile 为 null。register 校验现有实例的配置与资源绑定，use 校验配置路径存在；都不打开模型连接或启动 Runtime。
