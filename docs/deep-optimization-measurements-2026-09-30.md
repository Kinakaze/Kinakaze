# 深度优化构建与短程实测

已完成 Release 构建、73 项原生测试、3 项客体检查，以及两轮交错的基线/候选短程测速。缺失路径查询的改善最明确：20,000 次 stat/lstat 的两次中位数从 **1,523.263 ms 降至 497.484 ms，耗时减少 67.34%**。

本轮采用快速收尾范围，**没有重新运行完整 Node/npm 安装**。以下数字不能转换成完整安装提速比例。测速期间宿主仍有其他构建，账户查询和 fork 的结果波动明显。

## 修改与产物

本轮新增的 PE 解析优化位于 [`crates/bridge/src/native.rs`](../crates/bridge/src/native.rs)：

- 在一次不可变映像解析内记住最近使用的文件节，减少逐符号查找；每次命中仍检查边界。
- 先验证完整的导出地址表、名称指针表与 ordinal 表，再按固定宽度遍历。
- 保留所有导出的 UTF-8、排序、重复、ordinal、地址范围及可读性检查，没有引入跨调用或跨进程缓存。
- 新增跨节名称/地址交替、命中后名称越界的回归用例。

候选同时编译和验证了工作区已有的 stat/lstat 单次缺失结果传递、账户按需解析与枚举复用、ELF 格式探测句柄复用等修改。它是这些修改的组合版本，不能把整个测速差异归到 PE 解析这一项。

固定产物位于：

| 目录/文件 | 用途 |
| --- | --- |
| `artifacts/deep-eval-20260930/candidate-dist/` | 本轮 Release，含 init.exe、worker.exe 和 29 个 native 模块 |
| `artifacts/deep-eval-20260930/control-dist/` | 单独保存的基线二进制 |
| `artifacts/deep-eval-20260930/source/` | 候选构建使用的 1,657 文件源码快照 |
| `candidate-source-manifest.json` | 候选源码 SHA-256 清单 |
| `candidate-dist-manifest.json`、`control-dist-manifest.json` | 两组产物各 60 个文件的 SHA-256 清单 |
| `candidate-build.log` | 完整构建和导出检查记录 |

候选源码在构建后核对未变，两组二进制在测速后核对哈希未变。工作目录还存在其他并行修改，实测对象以这些固定产物为准。

基线来自已生成的 `artifacts/io-deep-20260930/baseline-dist`，复制前后与副本哈希一致。其原源码目录后来继续发生编辑；保留了基线原始源码哈希清单，`source-comparison.json` 记录哪些文件已无法从该目录恢复原内容。因此 `runtime-changes.diff` 只包含可按原哈希复核的部分差异，不能作为完整的版本差异。

## 验证

| 验证 | 结果 |
| --- | --- |
| Release 全工作区构建与 native 导出检查 | 通过 |
| Bridge/PE 原生测试 | 20 通过 |
| stat 缺失结果、回写、安装操作、原子创建、verity、native lookup | 25 通过 |
| 账户查询与枚举 | 28 通过 |
| NativeLookupProbe | 通过 |
| SparseForkStackProbe | 通过 |
| AllocationGrowthProbe | 通过 |

原生测试合计 **73 通过、0 失败**。记录为 `bridge-tests.log`、`native-tests.json` 和 `guest-tests/results.json`，均在 `artifacts/deep-eval-20260930/`。

测试覆盖了缺失名称随后创建、symlink、合成配置文件、verity/持久化边界、账户解析与枚举重置，以及 fork 栈和内存增长。完整安装、完整兼容性矩阵和长期泄漏测试不属于此次验证范围。

## 两轮原始测速

使用 [`tools/benchmark-deep-paths.py`](../tools/benchmark-deep-paths.py)，执行顺序为 **基线→候选→候选→基线**。两个 root 分别从同一 seed 独立安装，没有共享可写 inode。每次启动独立 session，使用两个预热 worker，关闭启动/加载/I/O 诊断。

表中是 guest 内计时，排除 Python 启动；完整调用墙钟、Job CPU 和资源样本另存于报告。每次运行还检查 ENOENT 后创建能够立即被 stat 看见、账户枚举可重置，以及 128 个子进程全部正常退出。

| 轮次 | 版本 | 20,000 次缺失 stat/lstat | 20,000 次账户查询 | 128 次 fork/wait |
| --- | --- | ---: | ---: | ---: |
| 1 | 基线 | 1,489.078 ms | 3,141.432 ms | 6,249.896 ms |
| 1 | 候选 | 473.547 ms | 2,932.458 ms | 6,318.874 ms |
| 2 | 候选 | 521.421 ms | 3,152.148 ms | 7,414.203 ms |
| 2 | 基线 | 1,557.448 ms | 4,319.653 ms | 8,294.379 ms |

| 两次中位数 | 基线 | 候选 | 耗时变化 |
| --- | ---: | ---: | ---: |
| 缺失 stat/lstat | 1,523.263 ms | 497.484 ms | −67.34% |
| 账户查询 | 3,730.542 ms | 3,042.303 ms | −18.45% |
| fork/wait | 7,272.137 ms | 6,866.538 ms | −5.58% |

缺失路径查询两轮都明显缩短，符合消除重复 native 打开的预期。账户查询的中位数下降，但样本少且基线第二轮变慢。fork 第一轮候选略慢、第二轮较快，**不足以认定稳定提速**。`timing-environment.json` 记录了宿主并行编译的情况，不把这些结果表述为隔离环境下的性能保证。

原始结果为 `artifacts/deep-eval-20260930/micro/report.json`，四次运行均通过。复现时使用新的输出目录：

```powershell
python tools/benchmark-deep-paths.py --control-dist artifacts/deep-eval-20260930/control-dist --candidate-dist artifacts/deep-eval-20260930/candidate-dist --control-root artifacts/node-install-20260930/deep-eval-control-root --candidate-root artifacts/node-install-20260930/deep-eval-candidate-root --output artifacts/deep-eval-20260930/micro-repeat --rounds 2
```

此前 468.685 秒的完整安装来自带诊断的旧版本，不与本表直接计算提速。完整安装的新成绩仍待实际重测。
