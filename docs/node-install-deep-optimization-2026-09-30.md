# Node.js 安装路径的第二轮优化（2026-09-30）

本轮以已移除 `io_profile` 的当前实现为基线，针对上一轮日志定位的重复原生调用继续优化。

## 改动

1. `stat/lstat` 继续传递路径解析阶段的 `native_missing` 结果。正常虚拟文件和 `/etc/hosts`、`/etc/resolv.conf`、`/etc/environment` 合成文件处理完成后，已确认不存在的原生叶节点直接返回 `ENOENT`，不再重复打开。该观察只在一次系统调用内有效，创建新文件后的查询仍重新解析。
2. 原生可写文件完成 `ensure_writable` 校验后，在描述符中保留此事实。该原生文件对象的写权限会阻止 fs-verity 启用事务取得 deny-write 句柄，因此普通写入无需反复查询权限、重新打开元数据句柄及读取 verity EA。该标记随同一个原生对象的副本传递；外部导入句柄仍执行完整检查，描述符替换使用新对象的标记。

## 比较方法

- 基线和候选版本从 `artifacts/io-deep-20260930/source` 的同一份源码快照构建为独立 release 分发目录；快照及二进制保存 SHA-256。候选版仅叠加本轮修改。
- 基线完整安装使用全新根目录及本地缓存 Debian 包，保留 maintainer scripts、triggers、同步写回、安装后 audit 和 Node/npm 功能验证。用户要求快速收尾后，未再开启候选版完整安装；两版性能比较限定为下述微基准。
- 微基准在持久 init 会话内交替运行新旧版本。计时范围为 Python 已启动后的系统调用循环；每种负载先预热，再比较多轮中位数。写入完成后验证内容和 fsync；缺失查询完成后创建文件并验证新查询结果。
- 不开启 I/O 日志或原生栈采样。测速期间观察到其他会话的编译活动，完整安装单次耗时必须结合微基准及环境干扰解读。

## 验证与结果

每项每版先预热一次，再交替测量 5 轮，每轮 5,000 次操作。表内为 guest 循环耗时中位数，排除程序启动时间。

| 负载 | 基线 | 候选版 | 耗时下降 |
| --- | ---: | ---: | ---: |
| 缺失文件 `stat` | 393.338 ms | 190.238 ms | 51.6% |
| 缺失文件 `lstat` | 553.417 ms | 225.082 ms | 59.3% |
| 原生文件 31 字节 `write` | 687.273 ms | 38.295 ms | 94.4% |

小块写入循环约为原先的 17.9 倍速度；该循环的 `fsync` 和完整内容校验在计时结束后执行并通过。此结果不代表持久化写入或完整安装提速 17.9 倍。

- 两版独立 release 分发构建成功；冻结源码的差异共 8 个文件，包含 3 个生产代码文件及相关测试。核对时这些文件与工作区一致。
- **55 项原生回归通过，0 失败**；另有 1 项已有原生能力诊断保持忽略。范围包含缺失路径、verity 事务与排他性、原子创建、`/proc/self/fd`、写回和位置写入。一个额外 duplication 过滤器匹配 0 项，不计入通过数；本轮新增 verity 回归和 guest 探针覆盖实际描述符复制与替换。
- **2 项 guest 探针通过**：合成配置文件在 chroot 后的 stat/lstat，以及可写句柄跨 dup/fork/exec、改名、删除、父进程关闭和只读槽位替换后的行为。
- Rust 格式检查、相关文件 `git diff --check`、三个 Python 探针语法检查通过。
- 基线完整安装 **357 个包，531.461 秒**，启动 5,255 个原生进程。Node.js `v18.20.4`、npm `9.2.0` 检查通过，`dpkg --audit` 和 `dpkg --verify` 结果为空。

本机其他会话的编译活动同时存在。交替顺序和中位数减小了顺序偏差，但不能视为独占机器测试；完整安装只有基线样本，暂不报告整套安装加速比例。主要剩余瓶颈仍需针对进程创建、fork 内存复制和 provider 初始化另行测量。

## 可复核产物

- [交替测速原始数据](../artifacts/io-deep-20260930/microbench/results.json)：每轮状态、guest/启动/CPU 耗时、探针与分发文件 SHA-256。
- [原生回归结果](../artifacts/io-deep-20260930/native-tests.json)和[guest 回归结果](../artifacts/io-deep-20260930/guest-tests.json)。
- [基线完整安装结果](../artifacts/node-install-20260930/deep-baseline/report.json)。
- [源码差异与哈希](../artifacts/io-deep-20260930/source-changes.json)、[候选版构建日志](../artifacts/io-deep-20260930/candidate-build.log)。
- 独立分发目录：`artifacts/io-deep-20260930/baseline-dist`、`artifacts/io-deep-20260930/candidate-dist`。本轮未覆盖其他会话使用的主分发目录。

复现微基准（输出目录需换成新的目录）：

```powershell
python tools/benchmark-descriptor-paths.py --root artifacts/node-install-20260930/deep-base-root --dist baseline=artifacts/io-deep-20260930/baseline-dist --dist candidate=artifacts/io-deep-20260930/candidate-dist --output artifacts/io-deep-20260930/microbench-repeat --modes missing-stat missing-lstat native-write --repeat 5 --warmup 1 --iterations 5000
```
