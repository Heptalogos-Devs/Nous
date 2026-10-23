# A_Memorix 记忆包 fixtures

三个 `.amembundle` 由 [MaiBot 的真实导出器](https://github.com/Mai-with-u/MaiBot/blob/c602a2780f4f4dda4a6be46d67abc1cfb1ad420c/src/A_memorix/core/runtime/services/bundle_admin_service.py) 生成。`generate.py` 从锁定提交的本地源码提取原始 `_export_bundle`、知识记录构造、NPZ 导出与 canonical 函数执行；测试适配器只提供选中的数据行、向量、图片和固定时钟。来源许可位于 `../../../licenses/A_memorix-LICENSE`，源码与包摘要记录于 `sources.json`。

| 文件 | 内容 |
| --- | --- |
| `v1-knowledge-1.0.0.amembundle` | 中文资料、实体、关系、真实 paragraph/graph NPZ |
| `v1-full-1.0.0.amembundle` | 上述内容及全部 15 个语义表，含 Episode、Fact、Profile |
| `v2-images-1.0.0.amembundle` | 完整语义、PNG、图片出现/证据/关联及真实 image NPZ |

`canonical-cases.json` 由同一上游 canonical 函数产生，覆盖 Unicode、控制字符、负零、浮点边界和固定种子的 256 个浮点位模式。图片包另含字符串编码的图片证据和 `1e-7`。

在仓库根目录重新生成，需已有 NumPy 和指定提交的本地 MaiBot 克隆：

```powershell
. ./scripts/dev-env.ps1
python -B crates/nous-app/tests/fixtures/bundles/generate.py --upstream-repo temp/repos/MaiBot
```

正常 Rust 测试通过 `include_bytes!` 读取已提交的包，无需 Python、模型凭据或临时克隆。包检查器验证 ZIP 全部成员、字节摘要、语义摘要及图片引用闭合，完整保留语义表和资产库存；`required_capabilities` 交给安装规划者匹配各数据 owner，向量数值与模型指纹兼容由相应向量 owner 负责。
