# APT 启动与账户查询热点优化（2026-09-30）

本轮继续分析已经完成的 Node/npm 357 包安装日志，没有再次运行 guest、安装测速或功能测试。历史诊断构建 `candidate-io1` 的安装耗时为 468.685 秒；这不是当前代码的性能结果。

## 安装阶段的归因

输入为 `artifacts/node-install-20260930/io1-analysis/all-events.tsv.gz`、`io1/session/profile` 与 `io1/report.json`。启动日志使用 `start_us` 对齐报告中的阶段 UTC 时间窗；跨阶段 span 按开始时间归类，累计 span 时长不等于串行关键路径。新输出保存在 `io1-analysis/vfs-reuse.json`。

| 历史安装阶段操作 | 次数 | 累计 wall time |
| --- | ---: | ---: |
| providers-total | 5,255 | 37.634 秒 |
| providers-discover | 5,255 | 20.251 秒 |
| providers-bind | 5,255 | 10.338 秒 |
| providers-shared-load | 5,255 | 3.483 秒 |
| worker-open-runtime | 5,255 | 23.363 秒 |
| guest-bootstrap | 1,914 | 10.834 秒 |
| worker-discover-modules | 5,255 | 1.508 秒 |

`providers-total` 包含 discover、bind、shared-load，不能把总计和子项相加。CPU 字段受 Windows 计时粒度影响；process CPU 还包含其他线程。`init-pool-wait` 是预热 worker 等任务的空闲 span，其中一个跨越整个安装阶段，不能归因为安装阻塞或累计到 apt 的耗时中。

完整 I/O 事件中，安装阶段解析 `/etc/passwd` 30,585 次、`/etc/group` 30,578 次，路径解析分别累计 3.929、3.662 秒。两者绝大多数来自同一 PID 37156 / TID 30100：各 **30,218 次**。此统计逐条扫描完整事件，没有采用路径排名的截断结果。对应代码的账户查询原先读取文件、解析所有记录，再从整表中查找一条；文件读取和字符串分配不包含在 `map.resolve` 的时长中。

因此，单纯优化账户枚举不能覆盖主热点；必须优化按名称或 UID/GID 查找。分析时安装根目录的 passwd/group 分别有 20、39 条有效非空记录，这只是当前文件内容，不保证等同于历史安装每一时刻的内容。

## 源码变化

### 复用已经验证过的 PE 导出信息

`crates/bridge/src/native.rs` 在 provider discovery 完成完整 PE 校验后，将 guest 导出保存为指向已固定只读文件的偏移索引。之后 materialize 直接使用此索引，省去第二遍 PE 头、节表、名称、ordinal 和地址扫描，也不用再次构造 engine alias 集合。普通 Rust 导出仍全部验证，但不复制名称、不长期保留其索引；engine alias、符号版本、运行时 ABI 标记和延迟 object layout 查询保留。

runtime 路径识别仍校验所有导出，但临时向量只保留三个识别标记。首次发现的完整校验、真正的 DLL 加载、初始化回调和对象布局查询仍然存在，不能宣称表中 20.251 秒或 10.338 秒已经全部省下。

索引与文件映射由同一个 `Arc<NativeImage>` 持有，文件保持原有拒绝写入/删除的共享方式，保证名称偏移在延迟绑定期间有效。最后一个 owner 释放时，索引、映射和文件句柄一同释放。没有添加跨进程原生指针缓存。

### ELF 格式探测与快照复用同一文件打开

`engine/crates/guest-engine/src/lib.rs` 保留直接启动时探测 shebang/ELF 的 `GuestImage`，随后的 ELF 快照消费这个 reader，避免关闭后再次按路径打开同一文件，也避免探测后 rename 导致快照来自另一个 inode。`snapshot` 仍重新定位到文件开头，并保留完整性验证、逻辑 EOF 和 init 共享镜像路径。

识别出 shebang 时立即释放脚本 reader，再打开解释器；exec handoff 已携带不可变镜像，继续复用这些字节。数据内容仍需要正确加载，格式探测也仍会读取文件头。这项优化省去重复 open，没有取消必要的加载步骤。

### 账户查询只解析匹配记录

`libs/libc/src/userdb.rs` 的名称、UID、GID 查询先扫描目标字段，只对匹配且格式有效的记录构造拥有字符串的账户对象。其余行不再产生字段向量、账户对象和各字段字符串。root 位于文件前部时也无需继续解析余下行。

每次 keyed lookup 仍解析当前 guest 路径并读取当前文件，保持文件内容修改、替换、namespace 变化之后查询结果更新的原有行为。可读但空或全部无效的数据库仍是权威结果，不注入默认账户；无法读取时保留原有 fallback。完整的 u32 UID/GID、跳过坏记录和可选 group members 语义保持。

这主要减少 CPU 和分配，不会直接消除上表账户查询的路径解析和读取次数，不能把 7.591 秒写成实测节省。

### 枚举持有一次快照并及时释放

`libs/libc/src/userdb/enumeration.rs` 在 getpwent/getgrent 的同一次枚举中只构造一次账户表；`ERANGE` 不消费当前记录，重复 EOF 不重新读取。EOF、set/end 操作释放表和字符串。枚举与名称/ID 查询不共用长期缓存。

快照不持有 native 文件句柄。fork handoff 仍只传递原有游标；子进程使用新建的本地枚举状态，不转移父进程原生堆指针或锁。新增互斥访问保证取记录、发布成功后推进游标及重置之间的顺序。

## 复现分析

```powershell
python tools/analyze-vfs-reuse.py --events artifacts/node-install-20260930/io1-analysis/all-events.tsv.gz --profile artifacts/node-install-20260930/io1/session/profile --report artifacts/node-install-20260930/io1/report.json --output artifacts/node-install-20260930/io1-analysis/vfs-reuse.json
```

工具现在额外输出账户查询的逐线程计数及分阶段启动汇总。本次处理 35,632 条有效启动 span；完整 I/O 调用归因未解析父 span 数为 0。分析结果也保留写入等其他热点：历史 `vfs.write` 后代中有 40,811 次 NtCreateFile，累计 2.874 秒，后续优化需对照当前 fs-verity 写入校验路径，不能把安全检查直接删除。

## 验证范围

release `cargo check --locked` 已通过 libc、runtime、worker、guest-engine 及相关依赖。另通过 libc、bridge、guest-engine 的 `cargo check --tests`，只编译测试代码，没有执行测试。新增回归用例覆盖 PE alias/版本及延迟 owner 生命周期、账户解析边界、枚举 ERANGE 重试和 EOF 释放。

编译日志：`artifacts/node-install-20260930/check-apt-hotpaths.log`、`check-apt-hotpaths-tests.log`。资源释放依据是所有权和错误路径审查，本轮未进行运行时泄漏检测，也没有发布新二进制或给出未经测量的提速比例。
