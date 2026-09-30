# 进程共享 pthread mutex

`pthread_mutexattr_setpshared(..., PTHREAD_PROCESS_SHARED)` 现在初始化实际可跨进程使用的 mutex，支持 NORMAL、RECURSIVE、ERRORCHECK、ADAPTIVE，以及 STALLED 和 ROBUST。getter/setter 在 libpthread 与 libc 中均有导出。共享属性、robust 属性和类型共用原有 4 字节属性对象，互不覆盖。

40 字节 mutex 对象保存创建进程 PID、创建时间和对象代次；native 命名 mutex 还按 manager domain 隔离。同一共享映射在不同地址访问时使用相同身份，重新初始化生成新代次。对象和 fork payload 均不保存 native handle 或 Rust 指针。所有者死亡后的 inconsistent/not-recoverable 状态留在共享对象中，使修复和毒化对其他进程可见。布局标志及修复常量参照 [glibc pthread 定义](https://raw.githubusercontent.com/bminor/glibc/master/sysdeps/nptl/pthreadP.h)，不声明与 glibc 的内部 robust list/raw futex 字格式互操作。

native 线程所有权和放弃检测使用 [Windows mutex](https://learn.microsoft.com/en-us/windows/win32/sync/mutex-objects)。ROBUST 所有者死亡后返回 `EOWNERDEAD`，当前拥有者可调用 consistent 修复；未修复的最终 unlock 将其变为 `ENOTRECOVERABLE`。STALLED 所有者死亡后继续忙碌或定时超时。线程退出时不主动释放死者持有的 mutex。最后一个 native handle 消失后，共享对象中的旧所有权仍用于识别死亡。

线程本地记录仅判断本线程是否拥有该代次，fork child 不继承父线程的递归拥有权。每进程 handle 缓存容量最多 128，持锁及等待线程另持有 Arc，缓存淘汰不会关闭它们使用的 handle。`KINAKAZE_PTHREAD_SHARED_CACHE_OPT=0` 关闭缓存，每次获取重新打开 native 对象；默认开启。超时走现有 event/timer 等待，保持绝对 deadline。私有条件变量与共享 mutex 组合走 event 后端，兼容停车开关关闭的模式。

验证：

- 独立 authoring worktree 的完整 `cargo build --workspace --release --locked` 通过，再按 Cargo JSON staging 全套 38 个二进制和 DLL，使用一致的 Rust ABI。
- 同一份冻结 native 测试在停车/共享缓存开关同时为 0、1 时均为 55 passed、2 个性能测试 ignored。
- 真实 Debian Python/C guest 的两种模式均通过：8 组类型/robust 组合的父子互斥与计数、两种时钟超时、40 组线程/进程死亡与修复/毒化、4 组 STALLED 死亡，以及共享文件两个不同映射地址与重新初始化。
- cleanup、robust condition cancellation、event timed/signal/fork、私有 robust 既有 guest 回归共 8 行全部通过。native export 工具在最终整合版本中 24 项测试通过。

配对性能测试使用同一冻结 native executable，每组 100,000 次无竞争 lock/unlock，预热一对，再交替执行 5 对。

| 模式 | 每次 lock/unlock 中位耗时 |
| --- | ---: |
| 每次重新打开 native 对象 | 3762.659 ns |
| 128 项 handle 缓存 | 414.594 ns |

该微基准约 9.08 倍；不代表应用整体吞吐或竞争锁的改善。初次测量与编译同时运行产生较大漂移，采用编译结束后的配对结果。原始数据、二进制/源码 SHA-256 和 guest 结果见 [测量记录](measurements/pthread-shared-mutex-2026-10-01.json)。复现：使用 `tools/stage-native-test-artifacts.py` 冻结 Cargo test JSON，再运行 `tools/benchmark-pthread-shared.py --native <frozen> --output <report>`；guest runner 选择 `--probe shared`。

本次完成进程共享 pthread mutex。跨进程条件变量已由后续的[共享条件变量实现](pthread-shared-condition-2026-10-01.md)补齐；PI 调度继承和原始 futex 锁字互操作进度另见对应阶段记录。

与 main 同时更新的 VFS exclusive-create 优化及 robust 别名生成规则已合并。合并后再次完整 release 构建并 staging 全套 DLL，10 行 pthread guest 回归及独立 robust aliases guest 验证全部通过；共享 getter/setter 的 GLIBC_2.2.5、libc GLIBC_2.34 和未知版本拒绝也由真实 guest dlvsym 验证。导出生成器在没有 import inventory 的情况下保留这些版本，参照 [glibc 的符号版本表](https://github.com/bminor/glibc/blob/glibc-2.36/nptl/Versions)。
