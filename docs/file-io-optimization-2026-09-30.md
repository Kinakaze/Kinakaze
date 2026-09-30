# 文件 I/O 映射、完整诊断与定向优化

后续更新：按用户要求，已删除运行时 `io_profile` 模块、全部调用插桩、fork/退出日志钩子及 `--io-trace` 采集开关。下文的日志格式与开关说明仅记录当时的诊断版本；当前源码不再提供此采集能力。历史日志和离线分析工具保留，文件 I/O 优化和默认双 worker 继续生效。

默认通用预热池改为 **2 个未执行 worker**，适用于 session-file 和显式指定 root/dist 的通用管道会话。显式 `--prewarm-pool` 仍可覆盖。池继续按需补充，已执行的 worker 不回收到空闲池。

2026-09-30 的 Node.js/npm 诊断安装得到 **10,059,240 条事件**、5,295 个线程日志文件，原始记录 **1,181,266,190 字节**。已插桩边界没有丢记录、路径截断、格式错误或缺少最终 checkpoint。Windows 重用了 PID：1,531 个不同 PID 不能当作进程生命周期数量；安装阶段 Job 统计为 **5,255 次进程创建**。

用户随后要求停止测试、直接优化。因此下述最终优化只做编译检查，**没有优化后的安装实测，也未完成最终版本的回归验证**。不要把旧诊断结果当作最终版本的性能结果。

## 日志与结论

全部文件位于 `artifacts/node-install-20260930/`：

| 文件 | 内容 |
| --- | --- |
| `io1-analysis/all-events.tsv.gz` | 198,765,372 字节的完整可读事件日志，保留每条记录与父子关联 |
| `io1-analysis/summary.json` | 分阶段/操作统计、路径汇总、fork 分段、原始文件 SHA-256 与完整性检查 |
| `io1-analysis/summary.md` | 可直接阅读的操作次数、总耗时、扣除子调用后的耗时 |
| `io1-analysis/flush-callers.json` | 完整刷新调用所属的 VFS 操作与耗时 |
| `io1/session/io-trace/` | 原始二进制日志与 fork 分段日志 |
| `io1/session/profile/` | 启动和加载阶段日志 |
| `io1/install/resources.jsonl` | 每秒 Job CPU/I/O、进程数、内存、线程数与句柄数 |
| `io1/report.json` | 二进制哈希、命令、阶段时间、包版本与校验结果 |

该次带诊断安装耗时 **468.685 秒**，357 个包，dpkg 解包阶段约 452 秒、配置约 9 秒。Node.js、npm、`dpkg --audit`、`dpkg --verify` 全部通过。诊断运行包含记录成本，不能与无日志运行直接比较提速比例。

安装阶段的主要已插桩开销：

| 操作 | 次数 | 累计时间 |
| --- | ---: | ---: |
| `CreateFileW` | 540,691 | 33.953 秒 |
| 直接调用 `NtCreateFile` | 386,817 | 28.524 秒 |
| `FlushFileBuffers` | 51,225 | 69.007 秒 |
| 其中来自 `sync_file_range` 的完整刷新 | 23,980 | **33.599 秒** |
| 其中来自 `fsync` 的完整刷新 | 27,245 | 35.408 秒 |
| 数据回写 `NtFlushBuffersFileEx` | 23,980 | 0.594 秒 |
| `NtQueryAttributesFile` | 753,500 | 9.332 秒 |
| `NtQueryEaFile` | 593,122 | 2.861 秒 |
| `process.fork` | 3,341 | 79.542 秒，含诊断外围成本 |
| `WaitForMultipleObjects` | 71,471 | 202.878 秒 |

等待、fork、VFS 操作与其子调用存在重叠，不能把上述时间相加当作总安装耗时。等待包含管道和子进程生产数据的时间，不能全部归为磁盘等待。完整刷新退化与大量元数据打开是可直接减少的开销；常规数据 `ReadFile`/`WriteFile` 提交分别约 1.18/2.12 秒，继续堆 SIMD 无法解决这些内核调用。

## 已实施优化

### 避免 chmod 后的数据回写退化

旧实现对每次 `sync_file_range` 都重新申请写句柄。dpkg 写完文件后设置权限，宿主 readonly 属性使重新打开失败，但原有写句柄仍拥有写权限。这次出现 **23,980 次 EACCES**，随后全部退回完整设备缓存刷新，单独消耗 33.599 秒。

新实现先固定描述符的 inode 和挂载状态，直接在原有打开对象上请求 `FLUSH_FLAGS_FILE_DATA_ONLY`。正常写描述符无需重新打开，chmod 后也不再经过这条退化路径。仅原句柄确实没有写权限时才尝试独立写打开；不支持数据回写标志的文件系统仍保留完整刷新的兼容路径。

`fsync`/`fdatasync` 的完整持久化屏障保留。`sync_file_range` 本来就不要求刷新设备易失缓存，Windows 的 DATA_ONLY 映射与此一致。依据：[Linux sync_file_range](https://man7.org/linux/man-pages/man2/sync_file_range.2.html)、[Microsoft NtFlushBuffersFileEx](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntflushbuffersfileex)。

共享文件对象上的完成状态只检查本次 I/O status block。异常 `STATUS_PENDING` 路径保留句柄和状态块直到完成，并用内核短睡眠避免忙等；不调用会取消同一文件对象其他 I/O 的 `CancelIoEx(handle, NULL)`。正常同步完成路径不等待、不轮询、不启动后台任务。该异常等待路径延迟信号处理直到请求退休。

### 复用操作内已有的元数据对象

- `MetadataHandle` 直接拥有 RAII `Object`，`fchmod`、`fchown` 借用它完成后续修改，减少第二次 native reopen/close。
- 所有权修改需要的 `READ_CONTROL` 在第一次 metadata open 一并申请，保持原有访问要求。
- `chown` 在同一 inode 锁内使用同一 EA 记录完成权限检查和更新，去掉第二次查询；仍执行 setuid/setgid 清除和 capability 移除。
- 不引入路径缓存、权限缓存或常驻句柄池；rename/unlink 后仍操作已固定的 inode，挂载写入守卫持续覆盖修改过程。

### 固定 fsync 的描述符生命周期

`fsync` 在描述符表锁下验证并复制句柄，连同挂载状态一起保留到刷新结束，避免并发 close/dup2 后使用已回收的裸句柄。`O_PATH` 返回 EBADF。信号在操作资源释放前延迟交付。

## 历史诊断版本的开关与格式（已移除）

当时版本默认关闭。设置宿主进程环境变量 `KINAKAZE_IO_TRACE_DIR` 为已存在的绝对目录即可启用；`KINAKAZE_IO_TRACE_LIMIT_MB` 限制每线程日志，默认 1024 MiB，可设 1–16384。必须在启动 init 前设置，环境快照会被子进程继承。

关闭时缓存开关后直接返回，不查询时钟、不构造路径、不访问线程日志状态，也不保存/恢复 Win32 LastError。启用后每线程约 64 KiB 批量缓冲，最大嵌套 128 层，记录路径上限 4096 字节；没有共享日志锁、后台写线程或跨操作保留的日志文件句柄。记录代码保存/恢复 Win32 LastError，避免改变被测 API 错误。fork 冻结分配器前先刷出缓冲并暂停记录，子进程按 PID 重建状态，不关闭继承来的父进程句柄值。

每条二进制记录使用 88 字节 little-endian 头：Python `struct.Struct('<IHH7Qq4I')`，后接 UTF-8 操作名和路径。

| 字段 | 解释 |
| --- | --- |
| size/op_length/flags | 完整记录长度、操作名长度、路径截断标志 |
| seq/parent | 当前线程文件内的操作 ID、父操作 ID |
| start_unix_ns/elapsed_ns | UTC 起点、单调时钟耗时 |
| arg0–arg2 | fd、长度、偏移、权限、父句柄等操作参数 |
| result | Linux 负 errno / 字节数 / fd，或原生 NTSTATUS / Win32 负错误码；未知值为 INT64_MIN |
| pid/tid/path_length/version | 宿主线程身份、路径长度、格式版本 1 |

`native.Nt*` 保留有符号原生状态，包括 STATUS_PENDING；完成与等待另计。`vfs.read/write` 保存实际字节数。移除前版本的 `vfs.open/openat` 保存返回 fd，首轮 io1 日志这两项仅保存成功/错误。rename 的路径字段用字面量 `\0` 分隔两个路径。fork 记录原始父参数，移除前版本成功结果为 child host PID；首轮日志结果未知。

`trace.flush` 单独统计诊断写入耗时；`trace.checkpoint` 保存丢弃数、写失败标志和未结束层数。日志是已列出的 VFS/原生边界的全量记录，并非 Windows 全系统 API 拦截：宿主库内部调用、未插桩模块和 fork 冻结区不在二进制记录覆盖范围内。不能用打开/关闭次数差值证明句柄泄漏。

路径统计内存有 20,000 个键上限，首轮有 1,189,680 条路径事件未纳入路径排行榜；它们仍完整保存在事件文件中。百分位使用二进制直方图的上界。强制终止、损坏或无法写入时必须结合最终 checkpoint 和文件错误判断完整性。

复用现有日志，无需重新安装：

```powershell
python tools/analyze-io-trace.py --input artifacts/node-install-20260930/io1 --output artifacts/node-install-20260930/new-analysis
```

## 验证范围与剩余差异

用户叫停前，诊断版本通过默认两个 worker 的预热/补充/崩溃替换/退出清理、三个 guest 兼容探针、6,984 条小规模 I/O/fork 日志完整性检查，以及上述完整安装校验。解析器的两项格式与损坏检测测试通过。最终回写和元数据优化没有再运行这些测试，也没有新的速度承诺。最终源码已通过 `cargo check --locked --release -p kinakaze-v2-libc -p kinakaze-v2-init` 和 diff 空白检查，记录保存在 `artifacts/node-install-20260930/check-io-final.log`。

现有目录 fsync 和宿主权限拒绝只读句柄的兼容行为仍不能声称完全等同 Linux 崩溃持久性；Windows 数据回写仍可能覆盖整个文件并同步等待，强于指定范围的异步提示。没有通过关闭 fsync、跳过 dpkg 钩子或省略错误处理来换取速度。

`candidate-io1` 是诊断采集时的冻结二进制。`candidate-io2` 属于中间打包产物，已标记不可作为最终已验证发行版；最新修改在源码中。
