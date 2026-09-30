# futex 等待、信号重启与本轮验证

后续已修正旧式超时入口顺序及所有等待 API 的安全 timespec 复制。
最新 605 项两配置回归、五类客体探针及 raw 202 测量见
[超时入口阶段记录](futex-timeout-entry-2026-10-01.md)。本页保留此前构建的证据。

raw 455 采用独立解析入口，支持 U32/private、位掩码及绝对超时，复用已有
私有和共享队列。值与掩码的 64 位参数先检查宽度；超时复制先于空掩码与键
解析。空超时不检查 clockid；值不匹配优先返回 EAGAIN。单次等待尝试使用
已复制的截止时间，键解析耗时包含在预算内。

本阶段修正了此前信号测试中的重启假设：旧式 FUTEX_WAIT/WAIT_BITSET 有
超时时，用户处理器使等待返回 EINTR，即使设置 SA_RESTART；旧式无超时等待
则按 SA_RESTART 决定是否重试。现代 raw 455 与 waitv 使用可重启系统调用
语义，重启时重新复制超时及向量描述符。因此，处理器修改或解除映射的参数
会在下一次尝试被观察；不变的绝对截止时间不会延长等待。

依据：[Linux 6.12 futex syscall 解析](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/syscalls.c)、
[等待返回与取消](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/waitwake.c)、
[x86 用户处理器的重启规则](https://github.com/torvalds/linux/blob/v6.12/arch/x86/kernel/signal.c)。

取消先退休队列成员、迁移目标及向量兄弟记录，再调用用户处理器；已经提交
的唤醒仍返回成功。原生测试覆盖初次复制后超时页解除映射、重启时重新访问
超时页、修改描述符值/flags/reserved/地址，以及旧式定时/无超时等待的两种
处理器配置。生产 semaphore 保留既有共享等待入口。

## 已完成验证

实现提交为 `8ec3cf3`、`6d8d0ca`、`8fde49d`。在独立审查工作树和 target 中
构建；依赖基线为 `1e050ec`，应用本阶段 futex 改动，保留该基线的 fork 注册
接口。下列结果验证这套固定构建，不能推断其他会话的全部未提交改动已通过。

- libc release 同一固定二进制：默认及 `KINAKAZE_FUTEX_OPT=0` 各 600 项
  通过、0 失败、14 忽略。忽略项为独立基准和原有助手。
- 不启用测试配置的生产 libc 检查，以及完整工作区 release 构建均通过。
- 同一完整发行快照中的真实 Debian scalar、signal、vector、requeue 探针，
  分别启动开/关优化的独立进程池，八个会话均通过。
- 覆盖清单工具三项检查通过；raw 455 关联实际 handler，保持路径证据待验证。

signal 探针通过真实 ctypes 用户回调、rt_sigaction 和定向 tgkill 检查
30 种组合；处理器自身唤醒原地址返回零，验证调用处理器前已退休记录。
scalar 包含参数/错误顺序、四种重排队 flags、共享文件不同 VA 的映射与 fork
唤醒。vector/requeue 再次检查 128 项混合、迁移索引及跨进程映射别名。

本地完整证据位于 `artifacts/syscall-all-paths-20261001/`：

- `restart-native-final.json`、`native-restart-final/manifest.json` 保存原生结果
  与测试程序、全部冻结依赖的哈希；运行前后固定产物一致。
- `restart-guest-staging.json` 保存成功 Cargo JSON 构建和 38 个镜像/宿主 std
  依赖的 SHA-256；`candidate-restart/` 为该构建冻结的独立发行目录。
- `guest-{scalar,signal,vector,requeue}/report.json` 保存客体结果和发行哈希。
- `restart-coverage.json`、`.csv` 保存覆盖清单及源文件哈希。

可移植的结果摘要、产物哈希和全部基准轮次提交在
[验证与测量记录](measurements/futex-wait-2026-10-01.json)。

## 性能测量的范围

在构建和客体检查结束后，顺序启动同一固定 native 二进制；一轮预热、五轮
交替开关，以下为中位数。raw 455 每次检查 ETIMEDOUT，并检查成员已退休。
每个进程使用独立 helper 域；背景为 16 个等待组共 2,048 个无关成员。

| 过期绝对超时登记/退休工作量 | 对照 | 默认路径 | 对照/默认 |
| --- | ---: | ---: | ---: |
| private，5,000 次 | 6.505 ms | 6.225 ms | 1.04 |
| 无标记，5,000 次 | 31.268 ms | 28.774 ms | 1.09 |
| 2,048 个无关成员，无标记 128 次 | 311.141 ms | 2.368 ms | 131.42 |

明显差异集中在背景成员较多时共享队列剪枝和批量退休。前两行的小幅变化需
更多测量；本表不测线程唤醒延迟、跨进程吞吐、Unix I/O 或端到端应用性能。
编译并行时取得的早期数据已标记为排除；客体会话耗时不作为性能证据。

```powershell
python tools/benchmark-futex-queues.py --case wait2 --binary artifacts/syscall-all-paths-20261001/native-restart-final/kinakaze_libc-39bf3e5edd681555.exe --output artifacts/syscall-all-paths-20261001/scalar-benchmark-final.json
python tools/test-futex-vector-guest.py --probe signal --root artifacts/goal-systemd-idle/debian-root --dist artifacts/syscall-all-paths-20261001/candidate-restart --output artifacts/syscall-all-paths-20261001/guest-signal
```

## 持续目标

全范围目标仍有效。375 项 syscall 共 3,750 项路径审查尚待完整证据；PI、
robust pthread、等待期间实时时钟变化、更多故障与生命周期组合，以及 VFS、
内存 I/O、Unix I/O 的逐路径兼容性和性能验证，仍是完成条件。
