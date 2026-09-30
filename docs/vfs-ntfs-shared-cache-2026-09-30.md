# NTFS 与 init 共享镜像：后续 VFS 优化

本轮只分析已保存的 io1 日志并修改源码，遵照要求没有运行安装、性能基准或测试程序。下列耗时属于旧诊断版本，不是修改后的成绩。默认两个预热 worker 继续沿用。

## 日志说明了什么

对 `artifacts/node-install-20260930/io1-analysis/all-events.tsv.gz` 按线程内父子 span 重新归因，安装阶段得到：

| 调用来源 | 子操作 | 次数 | 累计耗时 |
| --- | --- | ---: | ---: |
| 路径解析 | metadata open | 170,070 | 10.904 秒 |
| 路径解析 | NtQueryAttributesFile | 753,500 | 9.332 秒 |
| rmdir | CreateFileW | 87,705 | 4.878 秒 |
| openat，包括后代调用 | CreateFileW | 199,946 | 12.419 秒 |
| EA 查询 | NtQueryEaFile | 593,122 | 2.861 秒 |

父子层级的耗时相互包含，不能相加。尤其 `rmdir` 共 88,454 次，其中 **86,991 次为 ENOENT**、749 次为 ENOTDIR、357 次为 ENOTEMPTY、357 次成功。大量清理 dpkg 临时/备份名称的操作，在解析时已经确认不存在，之后仍进行类型检查和申请 DELETE 句柄，这是本轮直接消除的重复工作。

现有路径汇总中 `/etc/passwd` 和 `/etc/group` 分别解析 30,585、30,578 次，累计约 7.591 秒；其中 metadata open 约 4.274 秒。`libs/libc/src/userdb.rs` 的账户查询仍会解析路径、读取文件、完整解析账户表。这是值得继续处理的调用源，但跨调用复用账户数据必须覆盖内容修改、替换、权限及 namespace 变化，本轮没有加入可能返回旧账户记录的长期缓存。原路径汇总有键数量上限，此处只引用明确记录的路径，不把它当作全局完整排行。

另对全部 io1 阶段的 loader/execution 日志统计：

- `image-snapshot`：5,592 次，1.581 秒。
- `section-copy`：7,518 次，1.042 秒；`snapshot-share` 只有 9 次。
- 内容哈希：7,527 次，合计处理约 2.075 GB，0.574 秒。
- `libmd`、`libz`、`liblzma`、`libbz2`、`libzstd` 分别快照 1,133、741、736、736、735 次；当前文件大小约 47、121、190、75、764 KB，均低于旧缓存 8 MiB 门槛。

loader 日志缺少时间戳，因此这组数值包括下载、安装和校验等全部阶段；当前文件大小是分析时读取的值。共享镜像能减少重复内容复制、哈希和进程私有快照，但这些时间在旧日志中占比有限。剩余的进程创建/fork、必要的 fsync 和重复元数据操作仍需分别优化，不能把扩大共享缓存等同于整体安装已大幅加速。

## 本轮源码变化

### 单次操作复用 NTFS 不存在结果

`native_lookup::Lookup` 把完整路径查询的 `missing` 观察结果交给 `Resolved`。只在 `OBJ_DONT_REPARSE` 路径检查及父目录验证成功后标记。原生 remove 和 rename 的源路径消费该观察结果，直接返回 ENOENT，省去后续失败的类型查询和文件打开。

该结果只活到本次调用结束。其他进程随后创建同名文件时，下一次系统调用仍重新查询。新建文件继续由原子的 native create 决定是否重名。proc/cgroup 分派与挂载写入检查仍在返回前执行；复杂 symlink、挂载和无法确认的情况保留原有解析路径。

### 扩大 init 的共享镜像缓存

缓存准入范围统一定义在 `crates/protocol/src/image_cache.rs`，VFS 和 init 同时使用 **32 KiB–64 MiB**，覆盖频繁装载的小命令和库，同时避免为超限文件发送注定失败的 RPC。

init 保留一份只读共享 section 和完整 BLAKE3 摘要。身份仍使用卷号、128 位文件 ID、长度与 read oplock，命中必须重新确认该 inode 的 lease 有效；不会只凭路径或 mtime 复用。内容变化导致 lease 失效后重新读取，fs-verity 文件仍走校验路径。oplock 不支持、申请失败或缓存忙时保留原有本地快照路径。Windows 的 read oplock 通过异步完成通知失效，相关机制见 [Microsoft FSCTL_REQUEST_OPLOCK](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_request_oplock)。

每次命中仅检查选中条目的 lease，不再对所有条目执行内核等待查询；键查找仍为最多 128 项的内存扫描。缓存未命中时清理已失效条目并淘汰 LRU。小于 8 MiB 的读取在已有缓存线程内完成，哈希直接使用 BLAKE3 的 SIMD 实现；大文件保留有界并行读取和哈希。

ELF 共享映射路径的最小同偏移段也降至 32 KiB，继续检查映射边界、段重叠、BSS 和私有页比例。只有满足布局要求的 ELF 才直接使用共享 section 的写时复制执行视图；例如 dpkg-deb 的较大 BSS 仍使用通用映射，不能宣称所有小程序的执行页都已共享。

### NTFS EA 直接解码

`ea::read_decoded` 在已完成的 native 查询缓冲区上直接调用 inode/verity 解码器，去掉原来的 `Vec<u8>` 分配、复制、再解码。解码结果只能拥有自己的字段，不能把栈缓冲区引用带出调用。普通定长 inode 记录无需为 EA 字节分配；symlink 字符串仍正常拥有内存。

不超过 512 字节的 EA 写入也使用对齐栈缓冲；长记录按需分配。名称、长度、格式校验与异步完成/取消规则保留。EA 仍在 NTFS inode 上，未改用路径侧文件或 ADS 缓存。

## 资源与诊断边界

- init 缓存同时受 **256 MiB** 页计费预算和 **128 条目**约束，常驻条目对应最多 384 个 file/event/section 句柄；处理请求时另有少量临时句柄。
- 在创建新 section 前回收空间，避免替换期间缓存自身额外保留一个最大镜像。此预算不包含 worker 持有的视图、私有 COW 页面及其他运行时资源。
- 条目、section、视图、文件和事件维持 RAII 所有权；lease 回收会取消并等待该条异步请求完成，缓冲区不会先于 I/O 释放。
- lease 仍由 session 所有的长寿命线程发起，避免请求线程退出使 lease 意外取消；session 销毁会关闭队列、加入线程并释放缓存。
- `KINAKAZE_STARTUP_PROFILE` 启用时新增 `image-cache-hit`/`image-cache-miss` 记录用于计数；它们是所选分支的 span，不代表完整 RPC 延迟。默认诊断关闭时不写日志。

新增分析工具读取既有完整事件及 loader 日志，不启动 guest：

```powershell
python tools/analyze-vfs-reuse.py --events artifacts/node-install-20260930/io1-analysis/all-events.tsv.gz --profile artifacts/node-install-20260930/io1/session/profile --output artifacts/node-install-20260930/io1-analysis/vfs-reuse.json
```

输出包含安装阶段直接/间接调用归因、remove/rename 结果分布、全部阶段共享镜像候选及未解析父 span 数。与原完整事件文件一起用于继续研究，不以抽样替代原记录。

## 验证范围

已通过 release `cargo check --locked`：libc、init、link 及依赖 VFS；也通过 init、VFS、link 的 `cargo check --tests`，后者只编译测试代码，没有执行测试。日志分别为 `artifacts/node-install-20260930/check-ntfs-shared.log` 和 `check-ntfs-shared-tests.log`。

本轮没有运行泄漏测试、功能回归或速度对比，没有生成新的已验证发行包。资源释放结论来自所有权和取消路径的源码审查；实际提速幅度尚未测量。
