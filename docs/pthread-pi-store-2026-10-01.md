# pthread PI 生产路线固定与原子写入

在 `9007a9d` 上复测合并后的 PI 热路径：同一冻结二进制的五轮交替中位数，
无后台线程时优化路线约快 1.88–2.04 倍，32 个空闲 PI 线程时约快
12.62–14.82 倍。因此生产构建固定使用这条路线；`KINAKAZE_PTHREAD_PI_OPT`
仅在原生 cfg(test) 程序中保留对照含义。竞争、未完成 journal 和未恢复
优先级仍按实际图状态进入必要的恢复处理。

本轮进一步将 pthread PI 的无条件字段写入改为单条内存 XCHG。普通
lock/unlock 对更新六次计数、owner 和 users 字段，原来每次先通过
ReadProcessMemory 读取旧值，再循环 CAS。新实现保留原子 RMW 和内存屏障，
省去这六次显式预读及重试。所有权交接的条件 CAS 和 journal 保持原规则。

写入前仍检查页面可写性和 PAGE_GUARD，避免消费 guard。验证后发生保护
变化时，vectored handler 只识别对应汇编指令地址，返回 EFAULT；不会把
其他 guest 故障改为成功。读、条件 CAS 和写入的返回契约分别保留。

## 同二进制对照

固定之前胜出的 PI 算法，新增 `pthread-pi-store` 测量旧字段写法与 XCHG。
每种组合实际调用公开 pthread lock/unlock 500 对，局部预热 16 对；
完整预热一轮，再交替执行七轮并取中位数。后台线程实际存活、注册并通过
barrier 保持空闲；开始前检查没有其他活跃进程或无关 futex 记录。

| 组合 | 后台线程 | 旧写法 ns/对 | XCHG ns/对 | 耗时减少 |
|---|---:|---:|---:|---:|
| private stalled | 0 | 7050 | 5844 | 17.1% |
| shared stalled | 0 | 8035 | 6633 | 17.4% |
| private robust | 0 | 8331 | 7060 | 15.3% |
| shared robust | 0 | 8360 | 7177 | 14.2% |
| private stalled | 32 | 7085 | 5791 | 18.3% |
| shared stalled | 32 | 7725 | 6529 | 15.5% |
| private robust | 32 | 8214 | 7059 | 14.1% |
| shared robust | 32 | 8284 | 7052 | 14.9% |

八种组合均胜出。生产只编译 XCHG 字段写入；旧循环与
`KINAKAZE_TEST_PTHREAD_PI_STORE_OPT` 只存在于 cfg(test)。实际 release
libc 镜像确认不含上述两个对照开关的字符串。guest 工具继续比较 PARK 和
SHARED_CACHE 两个配置，PI 在两边均固定使用本轮生产实现。

这是本机 release 原生组件基准，不能推断 guest 应用或 syscall/VFS/I/O
整体吞吐。较早和较晚批次的机器负载、代码及频率不同，上表使用同二进制
交替对照。没有把两个不同批次的提升百分比直接相乘。

## 退出恢复与验证

早期候选有一次 robust 获取 journal 的 before-cas 恢复返回 ENOENT。
更快写入暴露出接收线程结束后 park 已关闭的时序：调用者之前的存活检查
不能保证 OpenEvent 时仍存活。现在只有 event 不存在且重新证实线程死亡时
才继续公布 journal；活跃或无法证明死亡的身份仍返回原错误，不制造唤醒。
保留首次失败日志，不把它计入最终通过数。

新增测试覆盖可写页的 32 位边界值、相邻字段、只读/NOACCESS、保留 guard、
直接故障指令续回，以及缺少 park 时活跃身份返回 ENOENT、死亡身份恢复。
最后冻结图的验证结果为：

- FUTEX_OPT × 测试 STORE_OPT 四组完整 libc 配置，各通过 653 项、忽略 25 项。
- PARK/SHARED_CACHE 两组完整 pthread 配置，各通过 59 项、忽略 4 项。
- 测试用旧 PI 路线分别运行两种写法，各通过 10 项、忽略 4 项。
- 连续 30 轮真实跨进程获取/死亡恢复，通过 210 个退出场景。
- 完整 workspace release 构建成功，同一 Cargo JSON 冻结 38 个构建产物。
- 22 行真实 Debian guest 回归全部通过，发行图哈希一致：共享 PI mutex、
  条件及 shared/private robust GNU 取消清理，raw PI/requeue PI、scalar、
  vector、普通 requeue、timeout 和 signal。

源码对应 `9007a9d88fbcc15a8d0a396acec978111be61511` 加本轮六个实现/工具
文件修改。源码、冻结依赖、构建日志、原始测量和全部结果的 SHA-256 见
[验证记录](measurements/pthread-pi-store-2026-10-01.json)。共享格式与代际
沿用 v6/v5；原先 [PI 接入](pthread-pi-mutex-2026-10-01.md) 的未完成边界
继续单独保留，总目标尚未全部完成。
