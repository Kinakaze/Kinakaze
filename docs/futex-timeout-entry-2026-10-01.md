# futex 超时入口与复制规则

本阶段修正旧式 raw 202 的公共参数处理，并使旧式等待、raw 455 和 waitv
共同使用可返回故障的 timespec 复制。原生及真实客体验证均已通过；完整 PI
锁后端仍待实现。

旧式入口现在先按命令复制超时，再检查操作 flags、掩码和地址键。WAIT 使用
相对时长；WAIT_BITSET、WAIT_REQUEUE_PI 和 LOCK_PI2 使用绝对时刻；LOCK_PI
隐含 realtime。非定时操作不会把第四个参数当指针读取。已复制的等待预算
包含键解析耗时，超时页后续解除映射不会使同一次尝试再次访问它。

| 输入 | 本阶段修正后的结果 |
| --- | --- |
| WAIT，错误地址与负 seconds | EINVAL，超时格式先检查 |
| WAIT_BITSET，错误地址、零掩码、可读超时 | EINVAL，掩码先于键检查 |
| WAIT_BITSET，零掩码、不可读超时 | EFAULT，超时复制先于掩码 |
| WAIT 加不支持的 realtime flag，超时不可读 | EFAULT，复制先于 flag 拒绝 |
| PI 定时命令，超时不可读或格式无效 | EFAULT 或 EINVAL，先于后端拒绝 |
| 过期超时、地址可读、值不匹配 | EAGAIN，值比较先于超时返回 |

timespec 复制使用本地结构和现有 ReadProcessMemory 包装，接受未对齐的
用户指针；保护页及部分复制返回 EFAULT。取消、信号处理及现代系统调用
重启规则继续遵循 [上一阶段记录](futex-wait-and-signal-2026-10-01.md)。

依据：[Linux 6.12 sys_futex 的复制与分发顺序](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/syscalls.c)、
[等待掩码、键及值比较](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/waitwake.c)。
waitv 的空向量检查先于超时复制；保护页测试使用有效描述符覆盖其复制阶段。

## 验证证据

不启用测试配置的生产 libc 检查及完整工作区 release 构建均成功。在独立
审查工作树中，依赖基线仍为 `1e050ec`，应用本阶段及此前 futex 改动；fork
注册接口保留该基线版本。原生最终构建显式使用完整发行构建的 Windows
feature 集合。每套产物均从各自成功的 Cargo JSON 图冻结全部依赖。

同一固定 libc 原生二进制，在优化开、关时各通过 605 项、0 失败、15 忽略。
新增五项回归检查错误优先级、非定时参数、未对齐超时、跨保护页复制以及
复制后解除映射和登记前延迟。确定性暂停检查相对预算不会在恢复后刷新。
首次验证中使用空 waitv 向量的错误测试已修正；失败记录保留，不作为通过证据。

真实 Debian 的 timeout、signal、scalar、vector、requeue 五类探针各在两种
配置下通过，共十个会话。新的 timeout 探针直接调用 raw 202/455/449，并用
mprotect 建立保护页；PI 用例只验证入口错误顺序，不证明锁操作已实现。

证据位于 `artifacts/syscall-all-paths-20261001/`：

- `deadline-native-final.json`、`native-deadline-final/manifest.json`：605 项两配置
  结果、最终测试二进制及完整依赖 SHA-256，运行前后固定文件一致。
- `deadline-guest-build.jsonl`、`deadline-guest-staging.json`：成功发行构建及
  38 个镜像/宿主 std 依赖；`candidate-deadline/` 是冻结发行目录。
- `guest-deadline-{timeout,signal,scalar,vector,requeue}/report.json`：十个实际
  客体会话及发行哈希。会话耗时包含启动，不作为性能证据。
- `deadline-coverage.json`、`.csv`：完整 syscall 覆盖清单和源文件哈希。

提交的 [验证与测量摘要](measurements/futex-timeout-entry-2026-10-01.json)
保留最终结果、产物哈希和全部基准轮次。覆盖工具三项检查及 Python 语法检查通过。

## 逐路径基准

构建、原生回归和客体会话结束后，顺序执行两项基准；每项一轮预热、五轮
交替开关，使用同一固定 release 二进制。每个子进程有独立 helper 域。
raw 202 新基准逐次验证返回值及零残留，覆盖 WAIT 相对超时、WAIT_BITSET
绝对超时和 private/无标记路径。

| raw 202 过期超时登记/退休 | 对照 | 默认路径 | 对照/默认 |
| --- | ---: | ---: | ---: |
| WAIT private，5,000 次 | 7.381 ms | 8.217 ms | 0.90 |
| WAIT_BITSET private，5,000 次 | 6.099 ms | 6.787 ms | 0.90 |
| WAIT 无标记，5,000 次 | 47.250 ms | 44.364 ms | 1.07 |
| WAIT_BITSET 无标记，5,000 次 | 43.520 ms | 40.395 ms | 1.08 |
| 2,048 个无关成员，WAIT 无标记 128 次 | 400.311 ms | 3.313 ms | 120.83 |
| 同上，WAIT_BITSET 无标记 128 次 | 386.401 ms | 3.273 ms | 118.07 |

raw 455 同步复测：private 5,000 次为 8.929/7.618 ms，无标记 5,000 次为
41.627/44.363 ms，背景成员下无标记 128 次为 409.123/3.745 ms。
对照与默认均使用本阶段的安全复制；这不是旧代码与新代码的性能对照。

显著差异集中在大量无关成员下共享队列剪枝和退休。普通 private 两行在本次
测量中默认更慢，普通无标记也没有一致改善；仍需定位复制和公共入口成本。
背景为原生等待组，不包括客体线程调度。本表不证明唤醒延迟、跨进程吞吐、
Unix I/O 或端到端系统加速。

```powershell
python tools/test-futex-vector-guest.py --probe timeout --root artifacts/goal-systemd-idle/debian-root --dist artifacts/syscall-all-paths-20261001/candidate-deadline --output artifacts/syscall-all-paths-20261001/guest-deadline-timeout
python tools/benchmark-futex-queues.py --case legacy-wait --binary artifacts/syscall-all-paths-20261001/native-deadline-final/kinakaze_libc-7f9765c78f27372e.exe --output artifacts/syscall-all-paths-20261001/deadline-legacy-benchmark.json
```

PI 的所有权、优先级传递链、重排队代理和退出清理，robust pthread，等待
期间实时时钟变化，以及全部 syscall、VFS、内存和 Unix I/O 的完整路径证据
仍是持续目标的完成条件。当前 375 项 syscall 的 3,750 项路径审查仍待验证。
