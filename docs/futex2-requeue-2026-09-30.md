# futex2 重排队与全部路径目标的本轮进展

本页保留 9 月 30 日的构建及测量记录。10 月 1 日补齐 raw 455，并修正
旧式定时等待与现代 futex 的信号重启区别；最新验证见
[等待与信号阶段记录](futex-wait-and-signal-2026-10-01.md)。

目标继续覆盖全部 syscall、VFS、内存 I/O 和 Unix I/O。当前仓库的 Linux
6.12 x86-64 LP64 表共 375 项；分发清单为 304 项已接入、1 项明确拒绝、
70 项缺少分发。3,750 项路径审查仍保留未完成状态，不能由公共桥或组件测试
推断为全部完成。

## 新重排队入口

raw 456 接入 `futex_requeue`，使用栈上两个 24 字节描述符，校验各自的
U32/private flags、reserved、值宽度和地址键。描述符复制先于有符号计数
校验；两个键解析先于源值比较。目标的值需要合法，但不参加比较。

源和目标可独立指定 private 或无标记操作，四种组合以及同一 VA 的标签转换
均已接通。PRIVATE 队列与无标记匿名内存保持隔离，不能合并。

普通 private 等待保留进程队列。发生跨后端迁移时，先发布桥接提示，再按
命名域锁、进程队列锁的顺序迁移；私有唤醒、wake-op 和旧式 requeue 在取得
进程队列锁后再次检查提示，关闭首次读取与发布之间的竞争窗口。每线程复用
命名 park 事件，桥记录仅含键、令牌、宿主线程和出生时间等值。

迁移保持原始等待令牌、位掩码、目标已有等待者的顺序和向量索引。取消跟随
令牌，不把迁移或提交前的 SetEvent 当成唤醒。崩溃 waker 的未提交通知不会
让等待者退回原队列。超时和信号处理先退休迁移目标及向量兄弟成员。
旧式定时等待遇到用户信号处理器返回 EINTR；旧式无超时等待及现代 futex
按 SA_RESTART 决定是否重启。现代等待重启时重新复制超时与描述符；
保持不变的绝对截止时间不会刷新预算。这一规则已于 10 月 1 日修正并复测。

private 键只校验对齐的用户地址范围，不强制查询页面；等待和源值比较仍需
读取内存。因此，可迁移到未映射的 private 目标并按键唤醒。当前地址窗口
采用 x86-64 四级页表的 TASK_SIZE_MAX。旧式 requeue 的负计数及 wake-bitset
的空掩码也在键解析前检查。

预先待处理、默认忽略的 SIGCHLD 使用现有 `signal::interrupt_pending()`
判断，不再因单纯存在 pending 位反复进退队。命名队列在零唤醒预算下直接
返回，避免纯迁移时进行无用的唤醒扫描。

依据：[Linux 6.12 futex2 syscall 解析](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/syscalls.c)、
[重排队与比较顺序](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/requeue.c)、
[私有键解析](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/core.c)、
[四级地址窗口](https://github.com/torvalds/linux/blob/v6.12/arch/x86/include/asm/page_64_types.h)。

## 9 月 30 日的验证（保留历史）

固定 release 产物通过 1,600 项原生回归：libc 591、VFS 854、guest-engine
60、kernel 95；34 项忽略项是基准、子进程助手或原有专项测试。关闭
`KINAKAZE_FUTEX_OPT` 后，同一 libc 二进制的 591 项再次全部通过。

证据为 `artifacts/syscall-all-paths-20260930/requeue-final-tests.json`。
`native-requeue-final/manifest.json` 保存测试程序及完整 SONAME 导入的 SHA-256，
运行前后固定文件一致。`tools/stage-native-test-artifacts.py` 只从成功 Cargo
JSON 构建冻结产物，不从其他构建目录拼接依赖。

专项包含四种迁移、掩码与计数、目标 FIFO、重复迁移、未映射私有目标、
超时、信号/重启、向量索引、桥提示竞争，以及实际子进程在提交前、部分银行
写入后、提交后死亡和另一域操作。新增确定性检查覆盖 pending 默认忽略
SIGCHLD 下的 private/无标记普通等待和 64 项向量等待。

完整工作区 release 构建成功，`tools/stage-guest-cargo-artifacts.py` 将同一
构建中的 38 个镜像/宿主 std 依赖保存到独立 `candidate-requeue`。真实 Debian
客体的 waitv 与 requeue 探针在开、关两种配置下均通过：

- `guest-requeue/report.json`：四种 flags 组合、同 VA 标签转换、未映射 private
  目标、向量索引、超时、共享文件映射别名、子进程唤醒父进程迁移后的私有等待，
  以及父进程将子进程共享等待迁至自身私有键后唤醒。
- `guest-vector-requeue/report.json`：128 项混合向量、共享映射别名和 fork 唤醒。
- `requeue-guest-build.jsonl`、`requeue-guest-staging.json`：完整构建及发行包镜像哈希。

上述路径均相对 `artifacts/syscall-all-paths-20260930/`。单次客体会话耗时
受启动和缓存影响，不作为性能对照。

## 实测范围与限制

同一固定二进制，顺序运行各基准；每项内部交替开关，一轮预热、五轮记录。
raw 456 逐次检查返回计数并在结束时确认全部成员退休。背景及迁移记录由
原生等待组建立，不包含客体线程调度。

| raw 456 工作量 | 对照 | 默认路径 | 对照/默认 |
| --- | ---: | ---: | ---: |
| 空 private 源，10,000 次 | 7.677 ms | 8.458 ms | 0.91 |
| 空无标记源，10,000 次 | 21.019 ms | 20.292 ms | 1.04 |
| 2,048 个无关成员，空源 10,000 次 | 117.235 ms | 61.214 ms | 1.92 |
| 128 成员迁移 100 次，含上述背景 | 19.596 ms | 18.526 ms | 1.06 |

`requeue-benchmark-final.json` 保存各轮及产物哈希。改善集中在无关成员较多时
的空源剪枝；私有空路径没有证明改善，仍需优化和更多测量。小幅变化不足以
推广到全部重排队路径。

raw 449 的过期绝对超时入队/退休基准同步复测：

| 工作量 | 对照 | 默认路径 | 对照/默认 |
| --- | ---: | ---: | ---: |
| 8 项私有，1,000 次 | 11.696 ms | 11.577 ms | 1.01 |
| 128 项无标记，256 次 | 3,353.097 ms | 100.012 ms | 33.53 |
| 32 项混合，1,000 次 | 475.100 ms | 80.642 ms | 5.89 |

证据为 `waitv-requeue-benchmark.json`。这组数值替代当前版本的旧组件测量，
不代表线程调度延迟、跨进程吞吐或端到端系统性能。早期并行启动两个基准
导致共享域争用，其数据保留在 `*-concurrent-excluded.json`，已明确排除。

完整 futex PI、robust pthread、更多故障/生命周期组合，以及所有 syscall
逐路径兼容性与性能验证仍是持续目标的完成条件。
