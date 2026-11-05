# 面向大语言模型的标识符后缀

## 结论

优先在以下三种固定规则中选择；这里的 token 数均只统计**后缀本身**，不加入命名空间或其他文本。

| 目标 | 后缀构造 | 随机空间 | 字符长度 | 实测平均 token |
| --- | --- | ---: | ---: | --- |
| 保留一定可读性、无空格 | 六个连续的四字母单元 | 60 位 | 固定 24 | 六种 tokenizer：7.00–8.45 |
| 追求跨模型 token 效率 | 五个变长 ASCII 单元，用空格分隔 | 63 位 | 可变 | 六种 tokenizer 的样本均为 5 |
| 追求固定字符长度 | 六个四字母单元，用空格分隔 | 65 位 | 固定 29 | 六种 tokenizer 的样本均为 6 |
| 针对 o200k 和 DeepSeek 极限优化 | 四个变长 Unicode 单元，用空格分隔 | 60 位 | 可变 | 两者的样本均为 4；其他模型见下表 |

以上每种格式各抽样 10,000 次，数字是实际样本结果，不是对全部后缀的穷举。固定词表、均匀 CSPRNG 采样和数据库唯一约束共同提供随机性与唯一分配；后缀不应用作认证凭据。

当后缀必须是无空格的裸字符串，且人类认读不是目标时，六个三字母单元连续拼接也是固定长度方案：18 个 ASCII 字符、60 位，六种 tokenizer 的样本平均为 6.43–7.46 token。

现有 Proquint 后缀使用 48 位随机数，分为三个 CVCVC 单元。每个单元编码 16 位，见 [Proquint 原始论文](https://arxiv.org/abs/0901.4016)。当前数据库通过唯一约束及冲突重试负责已写入地址的唯一性；新的后缀方案应继续复用这一责任。

## 格式与取舍

### 可读后缀

六个四字母单元从同一个 1,024 项词表均匀选择，每项 10 位，共 60 位。例如：

```text
lawswakeborncoopfeedante
```

按四字符边界可还原为 `laws wake born coop feed ante`。词表按英文频率、发音词典和 tokenizer 过滤，但仍包含专名、缩写和低频项；正式用于面向人的界面前应结合语境审核。英语词频只用于筛选词表，生成时各项等概率。

### 仅追求 token 效率

五单元 ASCII 方案用一个 2,048 项首单元表和一个 8,192 项后续单元表，取位数为 `11 + 4 × 13 = 63`。六单元固定长度方案用一个 1,024 项首单元表和一个 2,048 项后续单元表，取位数为 `10 + 5 × 11 = 65`。单元之间的空格是后缀的一部分，必须原样保存、复制和传输；shell 参数需引用整个字符串。

Unicode 方案的首项和后三项分别从两张 32,768 项表中选择，每项承载 15 位，共 60 位。示例 `lord 마 Driving Kind` 在本次 o200k 和 DeepSeek V4.1 单独分词中各为 4 token。Unicode 表含多语言片段和标点，系统必须支持原样存储、JSON 转义及读取，不应对后缀做大小写转换、空格折叠或其他清洗。ASCII 空格方案同样要求保留空格和大小写。未按人类内容可接受性筛选的词表可能生成冒犯性文本。

如果服务端能维护会话内映射，也可向模型暴露 `a`、`b` 等短别名，再解析回持久地址。所测六种 tokenizer 对单独的 52 个 ASCII 大小写字母均为 1 token。别名只在当前映射作用域内唯一，需处理映射生命周期、请求隔离与误选；工具 schema、对象列表和映射本身的 token 成本也需纳入实际比较。

## 实测结果

表格统计完整**后缀**，每种格式各生成 10,000 个样本。可读方案与 Proquint 使用种子 `20261105`；其余方案使用 `20261106`。表中为各 tokenizer 的平均 token 数；括号中的随机位数不同时，比较也包含随机空间的差异。

| 后缀格式 | 字符数 | o200k | cl100k | DeepSeek V4.1 | Qwen3 | Seed-OSS | Mistral 7B |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 现有 Proquint（48 位） | 17 | 7.688 | 8.210 | 8.082 | 8.210 | 7.946 | 9.796 |
| 十六进制（48 位） | 12 | 7.115 | 7.098 | 7.195 | 10.666 | 10.742 | 11.471 |
| UUIDv4（122 位） | 36 | 22.793 | 22.814 | 23.033 | 31.807 | 31.954 | 33.766 |
| 可读四字母×6（60 位） | 24 | 7.004 | 7.037 | 7.603 | 7.037 | 7.283 | 8.447 |
| 三字母×6，无空格（60 位） | 18 | 6.433 | 6.823 | 6.765 | 6.818 | 6.614 | 7.462 |
| 六个四字母单元，以空格分隔（65 位） | 29 | 6.000 | 6.000 | 6.000 | 6.000 | 6.000 | 6.000 |
| 五个变长 ASCII 单元，以空格分隔（63 位） | 17–42（样本） | 5.000 | 5.000 | 5.000 | 5.000 | 5.000 | 5.000 |
| 四个变长 Unicode 单元，以空格分隔（60 位） | 10–46（样本） | 4.000 | 4.486 | 4.000 | 4.192 | 4.158 | 6.781 |
| Base64URL（60 位） | 10 | 6.958 | 7.297 | 7.147 | 7.510 | 7.260 | 8.458 |
| 十九位十进制（60 位） | 19 | 7.000 | 7.000 | 7.000 | 19.000 | 19.000 | 20.000 |
| 十五位十六进制（60 位） | 15 | 8.871 | 8.844 | 8.965 | 13.334 | 13.426 | 14.173 |

Unicode 方案在本次样本中的 95 分位数：o200k 和 DeepSeek 为 4，cl100k 为 6，Qwen3 和 Seed-OSS 为 5，Mistral 为 9。六单元固定方案使用 60 位时仍为 29 字符，五单元 ASCII 方案使用 60 位时字符长度仍可变；相应样本在六种 tokenizer 中分别均为 6 和 5 token。

可读词表的 1,024 个独立单元在 o200k、cl100k 和 Qwen3 中各为一个 token，DeepSeek 为 857 个，Mistral 为 729 个。连接符与相邻字符会改变分词；例如六个可读单元逐个用连字符分隔时，DeepSeek 的后缀平均为 10.190 token，而连续拼接平均为 7.603。完整地址实际放入消息时的邻接内容也可能改变边界，应在相应使用场景单独测量。

实测 tokenizer 为 `tiktoken==0.12.0` 的 o200k_base、cl100k_base，以及 `tokenizers==0.22.1` 加载的 [Qwen3-8B](https://huggingface.co/Qwen/Qwen3-8B)、[Mistral-7B-v0.1](https://huggingface.co/mistralai/Mistral-7B-v0.1)、[Seed-OSS-36B-Instruct](https://huggingface.co/ByteDance-Seed/Seed-OSS-36B-Instruct) 和 [DeepSeek-V4.1-Flash-1017](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash-1017) 官方 tokenizer。Hugging Face 测量关闭额外 special tokens，不套用 chat template。[DeepSeek-V3](https://huggingface.co/deepseek-ai/DeepSeek-V3)、[V3.2](https://huggingface.co/deepseek-ai/DeepSeek-V3.2)、[V4-Pro](https://huggingface.co/deepseek-ai/DeepSeek-V4-Pro) 和 [V4.1-Flash](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash) 对可读方案样本得到与表中相同的 token 数分布。

所用 Hugging Face 资源修订：Qwen3 `b968826d9c46dd6066d109eabc6255188de91218`，Mistral `27d67f1b5f57dc0953326b2601d68371d40ea8da`，Seed-OSS `1f44e2ee2b9d3b6a953c88afb2c84b4110635e73`，DeepSeek V4.1-Flash-1017 `1bae0e1088993b9b82de08f59de5be5a34e42170`。模型实际复制或选择后缀的准确率尚未测量。

## 词表与控制 token

所有选择以已下载 tokenizer 的**普通文本 token**为候选，不使用模型内部控制 token。构造过程排除了 o200k special token ID；随后验证目标 tokenizer 实际编码结果，并检查完整样本是否出现各 tokenizer 的 special 或 added token ID。检查范围包括 Hugging Face tokenizer 中未标记为 special 的 added token。

所测 tokenizer 的所有 added/special 标记字面串均包含 `<`、`>`、`|` 或全角 `｜` 中至少一个字符；Unicode 候选排除这些字符，ASCII 方案只含英文字母和空格。候选还排除 Unicode 空白和控制类字符，以及 NFKC 规范化会改变的单元。加入新 tokenizer 时应重新核对其标记和实际分词行为。

词表选择方式：

- **可读词表**：`wordfreq==3.1.1` 的 `top_n_list("en", 500000)` 中，保留 `[a-z]{4}`、出现在 `cmudict==1.1.1`、单独为 o200k 和 cl100k 单 token 且连字符加单元在两者均不超过 2 token 的项目；排除 `shit fuck rape porn cock anal anus coon cunt piss slut whore nigger nazi mong`，按原顺序取前 1,024 项。该机械过滤未覆盖所有内容问题。
- **跨模型 ASCII 词表**：分别选择在六种 tokenizer 下单独为一个普通 token 的首单元，以及空格加单元为一个普通 token 的后续单元。固定长度表使用四字母项；变长表使用小写字母项。候选按长度、o200k 排名和词面排序。
- **Unicode 词表**：首单元及空格加后续单元均在 o200k 和 DeepSeek 中单独为一个普通 token；按其余四种 tokenizer 的总 token 成本、长度、o200k 排名和词面排序，各取前 32,768 项。
- **无空格三字母词表**：对 o200k 三字母普通 token 候选，测量其与字母 `a`、`n`、`z` 相邻时在六种 tokenizer 中的成本，依成本与 o200k 排名选择 1,024 项。

这些词表已经固定在 [codebooks.json](codebooks.json) 中；顺序是编码合同的一部分。已分配后缀继续使用同一表，不在运行时重新下载 tokenizer 或按词频变更表项。

## 随机性与碰撞

在同一唯一性作用域内独立均匀生成 `n` 个 `b` 位随机值，至少发生一次原始抽样碰撞的概率近似为 `1 - exp(-n*(n-1)/(2*2**b))`：

| 抽样次数 | 48 位 | 60 位 | 63 位 | 65 位 |
| ---: | ---: | ---: | ---: | ---: |
| 1,000,000 | 0.1775% | 0.0000434% | 0.00000542% | 0.00000136% |
| 10,000,000 | 16.275% | 0.004337% | 0.000542% | 0.000136% |
| 100,000,000 | 近 100% | 0.4327% | 0.0542% | 0.0136% |

碰撞重试应与数据库唯一约束处于同一分配路径；多实例数据合并时也要处理跨实例冲突。词表语法校验没有校验和，字符错误仍可能形成另一个合法后缀。

## 豆包 tokenizer

未找到 Seed-2.1 Lite 或 Mini 的官方可下载 tokenizer。上表的 Seed-OSS 是字节的独立公开模型，其结果不能当作 Seed-2.1 Lite 或 Seed-2.0 Mini 的测量结果。

火山方舟提供需要 API Key 的[分词 API](https://docs.volcengine.com/docs/ark/tokenization-api?lang=zh)，可提交具体模型和后缀字符串，返回 token ID 和数量。文档提示真实调用可能产生费用，本研究未调用它。以下示例直接比较两个后缀：

```powershell
$body = @{
    model = 'doubao-seed-2-1-lite-260915'
    text = @('hubal-tumun-dukub', 'lawswakeborncoopfeedante')
} | ConvertTo-Json -Depth 4
Invoke-RestMethod -Method Post `
    -Uri 'https://ark.cn-beijing.volces.com/api/v3/tokenization' `
    -Headers @{ Authorization = "Bearer $env:ARK_API_KEY" } `
    -ContentType 'application/json' -Body $body
```

应核对返回的实际模型版本，并以成批随机样本比较；两条字符串只用于检查接入。

## 实现

[identifiers.py](identifiers.py) 和 [codebooks.json](codebooks.json) 构成可复用示例，不依赖 tokenizer 或其他第三方包。初始化时检查所用词表的 SHA-256 和唯一性；`generate()` 使用 CSPRNG，`encode()` 与 `decode()` 可逆且严格校验格式。

```python
from identifiers import IdentifierCodec

codec = IdentifierCodec("readable60")
suffix = codec.generate()
assert codec.decode(suffix) >= 0
```

命令行示例：`python identifiers.py readable60`。其他配置为 `no-space60`、`fixed65`、`variable63`、`unicode60`，以及用于相同 60 位对比的 `fixed60`、`variable60`。示例只生成后缀；具体名称空间和数据库写入由使用方负责。

复测时可用固定随机种子生成样本，对每个后缀直接调用指定 tokenizer，计算均值和分位数；不要用伪随机生成器分配正式地址。词表中携带的数据源及许可如下。

## 数据来源与许可

词表改编自 Robyn Speer 的 [wordfreq](https://github.com/rspeer/wordfreq)，使用其 3.1.1 英语频率排序并按上述规则选择、过滤；其数据按 [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) 使用。参考引用：Robyn Speer (2022), *rspeer/wordfreq: v3.0*, [doi:10.5281/zenodo.7199437](https://doi.org/10.5281/zenodo.7199437)。wordfreq 数据来源包括 [Google Books Ngrams](http://books.google.com/ngrams)、[Leeds Internet Corpus](http://corpus.leeds.ac.uk/list.html)、[Wikipedia](https://www.wikipedia.org/)、[ParaCrawl](https://paracrawl.eu/)、[OpenSubtitles](https://www.opensubtitles.org/) 和 SUBTLEX。SUBTLEX 由 Marc Brysbaert 等作者整理，是[免费提供的数据](http://crr.ugent.be/programs-data/subtitle-frequencies)；完整来源与致谢见 [wordfreq NOTICE](https://github.com/rspeer/wordfreq/blob/master/NOTICE.md)。

候选交集使用了 Carnegie Mellon University 的 [CMU Pronouncing Dictionary](https://github.com/cmusphinx/cmudict)。其许可声明如下：

> Copyright (C) 1993-2015 Carnegie Mellon University. All rights reserved.
>
> Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:
>
> 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer. The contents of this file are deemed to be source code.
> 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
>
> This work was supported in part by funding from the Defense Advanced Research Projects Agency, the Office of Naval Research and the National Science Foundation of the United States of America, and by member companies of the Carnegie Mellon Sphinx Speech Consortium. We acknowledge the contributions of many volunteers to the expansion and improvement of this dictionary.
>
> THIS SOFTWARE IS PROVIDED BY CARNEGIE MELLON UNIVERSITY ``AS IS'' AND ANY EXPRESSED OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL CARNEGIE MELLON UNIVERSITY NOR ITS EMPLOYEES BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
