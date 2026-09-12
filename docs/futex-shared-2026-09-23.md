# MAP_SHARED 跨进程 futex（2026-09-23）

普通 futex 现在使用真正的内核域共享等待队列，支持不同 worker 进程和不同虚拟地址访问同一共享对象。实现位于 `libs/libc/src/futex.rs`，映射键解析位于 `fdio.rs`，系统调用分派位于 `sysadmin.rs`。

## 支持的操作

| 操作 | 行为 |
| --- | --- |
| WAIT / WAKE | 锁内比较并入队，按实际选择的等待者返回唤醒数 |
| WAIT_BITSET / WAKE_BITSET | 位掩码筛选，单调时钟或实时时钟绝对超时 |
| REQUEUE / CMP_REQUEUE | 跨进程迁移等待记录，保留位掩码；比较失败无唤醒、无迁移 |
| WAKE_OP | 对第二个地址原子修改，按旧的有符号值比较并唤醒两个队列 |

不带 PRIVATE_FLAG 的私有内存也进入内核域队列，但键包含进程创建身份和地址，因此不会跨进程匹配。这样私有地址和共享地址之间的两地址操作仍由同一把锁保护。PRIVATE_FLAG 保留进程内快路径和独立键空间；fork 不继承父进程的等待记录。

## 身份、并发与生命周期

- 文件映射按卷身份、128 位文件 ID 和精确字节偏移匹配，支持硬链接、独立 open/mmap、非 64 KiB 对齐的映射偏移及关闭 fd 后的视图。MAP_PRIVATE 文件视图保持私有。
- 匿名共享 section 的身份存放在受管理的 backing 元数据中，fork 复制该身份并保留同一个原生 section。新建对象不会因为地址或句柄复用而匹配旧对象。
- 入队和唤醒在同一命名互斥锁下完成，遵循 [Linux futex 的检查/入队顺序](https://github.com/torvalds/linux/blob/v6.18/kernel/futex/waitwake.c)。两个队列的重排队与原子 wake-op 也在该锁下完成。
- 等待记录只有对象键、位掩码、唯一 token 和原生线程身份，不包含跨进程指针。超时和信号按 token 撤销，因此重排队后仍能准确清理。选中的唤醒优先于超时/信号；执行客体信号处理器前撤销外层等待，SA_RESTART 保留原超时预算并重新解析地址。
- 原地址在等待期间被 munmap、复用后，旧等待仍可经原对象的另一个映射唤醒。休眠期间不保留对客体 futex 字的 Rust 引用。
- 队列采用双缓冲提交；通知在持锁时先发送，队列随后发布。等待者取得锁后才确认选择结果，能恢复唤醒进程在提交前、写入未发布缓冲区中、提交后退出所留下的 abandoned mutex。
- 以线程创建时间和进程 ID 防止线程 ID 复用；已退出的等待者不会计入唤醒/迁移数。对象名包含内核域 epoch，不同管理器实例相互隔离。

## 验证与重现

最终发布包为 `artifacts/futex-shared-final-dist`，构建及导出检查通过。5 项客体探针全部退出 0；8 项 futex 单元测试通过。证据为 `artifacts/futex-shared-final-probes/results.json`、同目录探针日志、`artifacts/futex-shared-unit/futex-final.log` 和 `artifacts/futex-shared-final-build.log`。未运行整仓全量测试，也未在本轮重新验收 Minecraft 完整游玩。

```powershell
./tools/build.ps1 -Release -SkipFormat -SkipTests -DistDirectory artifacts/futex-shared-final-dist
python tools/test-runtime-compatibility.py --root artifacts/gnome-startup-root --dist artifacts/futex-shared-final-dist --output-dir artifacts/futex-shared-final-probes --timeout 180 --probe FutexUnflaggedProbe --probe FutexSharedProbe --probe XVisibilityProbe --probe GLStorageProbe --probe MixedXlibXcbProbe
```

`FutexSharedProbe` 使用真实 Linux fork/exec、mmap 和 futex syscall，覆盖共享匿名内存、文件硬链接/偏移/别名、memfd、PRIVATE 隔离、精确计数、混合私有/共享重排队、超时、信号重入与重启、SIGKILL 清理、地址复用和 150 轮父子进程握手。C 辅助库提供真实客体信号处理器。

Rust futex 测试另覆盖原子操作和有符号比较矩阵，以及真实 Windows 子进程强制退出后的事务恢复和内核域隔离。子进程辅助测试标为 ignored，由主测试显式启动，不是遗漏验收。

## 范围

本次实现的是上述普通、非 PI futex 操作。PI 优先级继承、robust-list 锁所有者死亡恢复、futex_waitv 仍未实现；清理死亡等待者不等同于 robust mutex 恢复。`mremap` 仍是仓库原有的未实现接口，本次未改变它。

现有匿名共享 MAP_FIXED 还要求原生分配粒度对齐的映射基址；地址复用测试在该支持范围内进行。文件 offset 为 4 KiB 的不同地址共享 futex 已单独验证。

每个内核域最多容纳 32,768 个同时等待记录，入队前回收死亡记录，耗尽时返回 ENOMEM。队列在单个命名互斥锁下串行化，未声称达到 Linux 分桶 futex 的吞吐量。无效指针和只读 WAKE_OP 目标会检查并返回错误；与其他现有客体内存系统调用一样，尚未提供针对恶意并发解除映射的原生异常屏障。

支持的命令及不支持的 CLOCK_REALTIME 组合依据 [Linux futex 分派](https://github.com/torvalds/linux/blob/v6.18/kernel/futex/syscalls.c)；无效位掩码返回 EINVAL，不支持的命令/时钟组合返回 ENOSYS。
