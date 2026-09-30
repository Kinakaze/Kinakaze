# Node 安装历史日志：剩余优化点

本次重新解析 `artifacts/node-install-20260930/io1/session/io-trace/`，并对照当前源码。没有重新安装、测速、开启运行时日志或修改运行时代码。

全程共有 10,059,240 条事件，安装阶段 6,998,039 条；本次二次分析的安装事件数与原始汇总一致。旧诊断安装耗时 468.685 秒。所有下列数字均来自这个旧版本，**热点累计耗时不等于修改后的可节省时间**。同一调用的父子层、多个进程的等待，以及启动与 fork 阶段存在重叠。

## 建议优先级

| 顺序 | 当前仍存在的路径 | 旧日志证据 | 建议 |
| --- | --- | --- | --- |
| 1 | `stat/lstat` 丢失本次解析的 missing 结果 | `lstat` 的 28,283 次 ENOENT 累计 3.817 秒，其中再次失败的 `CreateFileW` 为 1.938 秒 | 将 `native_missing` 带入 `StatResolution`，在普通 native 路径查询上直接返回 ENOENT |
| 2 | 每个进程重新发现 provider | 安装阶段 5,255 次；发现累计 20.251 秒 | 同一映像内导出索引复用已实现；剩余方向是跨 worker 复用按可靠模块身份固定的不可变目录/声明 |
| 3 | 每次普通文件 write 重开元数据查询句柄 | 40,811 次 `0x88` reopen 累计 2.921 秒，底层 `NtCreateFile` 2.874 秒、EA 查询另计 0.229 秒 | 研究把 writable/verity 检查结果绑定到已验证的可写 open description 生命周期，减少每次 write 的 reopen |
| 4 | fork 每次新建并启动宿主进程 | 全阶段 3,354 次：CreateProcess 16.089 秒，ready 等待 30.904 秒 | 削减 fork 子进程启动中的重复 provider 工作；专用 fork bootstrap 预备机制属于后续架构方案 |
| 5 | fork 客体映射重建与复制 | 全阶段 mappings 22.022 秒，171,235 次 copy calls，复制约 22.55 GB | 深入普通私有映射的远程提交/复制路径；评估像现有 arena 一样用子进程独占 section 的本地 staging 传输，或扩大适用的快照覆盖 |

第一项边界较小，优先落地。第二项需要设计跨进程复用与模块失效；第三项需要先证明可写句柄生命周期内的 verity 状态约束。后两项潜在收益更大，但不能从现有累计时间直接推算加速比例。

## 1. Provider：每进程重复发现

安装阶段的启动日志：

| 阶段 | 次数 | 累计时间 | 每次均值 |
| --- | ---: | ---: | ---: |
| `worker-open-runtime` | 5,255 | 23.363 秒 | 4.446 毫秒 |
| `providers-total` | 5,255 | 37.634 秒 | 7.162 毫秒 |
| 其中 `providers-discover` | 5,255 | 20.251 秒 | 3.854 毫秒 |
| 其中 `providers-bind` | 5,255 | 10.338 秒 | 1.967 毫秒 |
| 其中 `providers-shared-load` | 5,255 | 3.483 秒 | 0.663 毫秒 |

`providers-total` 包含后面三个子阶段，不能再次相加。启动日志中的 `init-pool-wait=469.187 秒` 是包围命令执行的等待，不是池调度多耗了 469 秒。

代码定位：

- `crates/loader/src/providers.rs::load` 每次调用 `ModuleCatalog::discover`，随后绑定所需模块。
- `crates/bridge/src/native.rs::ModuleCatalog::discover` 枚举目录、打开固定的只读映像并调用 `selected_exports::<false>`。
- 复核时工作区最新版本的 `NativeImage` 已保存 `IndexedExport`，`DiscoveredModule::materialize` 通过 `self.image.exports()` 复用解析结果，不再重复扫描同一映像。这项应列为已处理。

剩余方向是在 worker 间按可靠的模块身份和替换失效机制复用不可变目录与声明，减少每个进程重新枚举、打开和解析模块的成本。进程相关的 DLL 地址、TLS、COPY 重定向和初始化仍分别处理。现有符号延迟解析、同一固定映像内索引复用已经存在，不能重复计作待实现项；旧日志的 10.338 秒绑定时间也不能当作当前版本残余耗时。

## 2. 缺失路径：删除/重命名已处理，stat 仍有遗漏

旧日志中：

- `rmdir`：86,991 次 ENOENT，14.021 秒；其中 58,069 次 `.dpkg-tmp`，28,208 次 `.dpkg-new`。
- `rename`：28,208 次 ENOENT，4.480 秒。
- `lstat`：28,283 次 ENOENT，3.817 秒。
- `stat`：7,773 次 ENOENT，3.108 秒。

**当前 `mount/overlay/mounted.rs::remove` 和 `rename_with_flags` 已使用 `native_missing` 提前返回**，不能把前两项再次列作待实现收益。普通 native 解析由 `native_lookup` 查询末级名称，并验证已存在的父目录；这是单次操作中的观察，不是跨操作负缓存。

仍存在的遗漏在 `stat_resolution`：它返回 `StatResolution::Native(resolved.path)`，丢掉 `resolved.native_missing`。`fs.rs::stat_path_resolved` 因而再次调用 `CreateFileW`。旧日志中每次失败的 lstat 都包含一次失败的末级属性查询、一次成功的父目录查询和一次失败的打开。最后这一步累计 1.938 秒；失败 stat 中 ENOENT/路径缺失的打开另占约 0.461 秒。

应保留本次解析的 missing 结果，但仍让 `/proc`、挂载穿越和 `/etc/hosts`、`resolv.conf`、`environment` 等合成文件走既有分支。不能直接在统一解析器中把所有缺失叶子变成错误，因为创建和合成文件需要这个状态。

## 3. 普通 write：verity 查询成本大于 EA 本身

`lib.rs::platform::write` 对普通文件调用 `check_file_write`；后者检查权限后调用 `fs/verity.rs::ensure_writable`。这个函数每次都先 `Object::reopen(FILE_READ_ATTRIBUTES | FILE_READ_EA)`，再查询 verity EA。

旧日志的 40,811 次对应元数据 reopen 用了 2.921 秒，查询 EA 本身约 0.229 秒。主要可削减点是重复打开，而不是解析 EA 字节。

可研究的方案是：在成功取得可写原生句柄并检查/恢复 verity 状态后，把已验证状态绑定到具体 open description，而不是路径或可复用的 fd 数字。现有 enable 事务通过 `reopen_deny_write` 排斥写者，提供了推导这一生命周期不变量的基础。实现前仍需覆盖导入句柄、dup/fork、描述符替换、可写映射及 PREPARING 恢复；不能直接删除每次写入的检查，也不建议无条件缓存“没有 verity”。

## 4. Fork：启动和映射比 arena memcpy 更值得投入

fork 文本日志没有阶段时间戳，因此下表使用全阶段 3,354 次；安装阶段的 VFS 日志单独计到 3,341 次。

| 段 | 累计时间 | 均值 |
| --- | ---: | ---: |
| 总 fork | 76.327 秒 | 22.757 毫秒 |
| prepare | 49.078 秒 | 14.633 毫秒 |
| 其中 CreateProcess | 16.089 秒 | 4.797 毫秒 |
| 其中 child ready | 30.904 秒 | 9.214 毫秒 |
| mappings | 22.022 秒 | 6.566 毫秒 |
| arena | 3.203 秒 | 0.955 毫秒 |
| freeze | 0.105 秒 | 0.031 毫秒 |

`prepare` 包含创建和 ready 等待；这两项也与 provider 启动时间重叠。`clone_parent_inner` 仍调用 `prepare_child`，后者直接 `CreateProcessW`，因此当前 init 的两个通用预热 worker 并没有自动消除每次 fork 的这条新建进程路径。

专用 fork bootstrap 若要预备，必须解决父进程动态模块清单、句柄继承、地址布局及未发布子进程的清理；不能直接把通用池大小调大当作已经解决。

映射统计中 `arena_mapped` 每次约 16 GiB 是保留地址范围，实际 `arena_used` 均值约 2.42 MB。不能解释为每次复制 16 GiB。日志中的 `copied_bytes` 总计约 22.55 GB，另有约 12.79 GB 被记录为 COW shared。源码已经存在 COW 及连续私有页合并复制。继续优化应集中于仍需远程提交/复制的映射类型，现有日志不足以进一步分离页扫描、远程提交和实际复制各占多少。

## 已处理项与不应误判的数字

- `sync_file_range` 的 23,980 次权限失败及退化完整刷新（33.599 秒）已有原句柄 DATA_ONLY 路径；保留真正 `fsync` 的持久化行为。
- 元数据句柄在 chmod/chown 内的复用、独占创建时初始 EA 的原子写入、部分重复 stat 查询、image snapshot 共享及 provider 同一映像内的导出索引复用，在当前源码中已有相应处理。这次历史日志没有测到这些最新改动。
- `WaitForMultipleObjects` 的安装累计 202.878 秒不是 202.878 秒磁盘延迟。本次关联到 write 下的等待为 108.390 秒、read 下为 56.613 秒；其中返回 32 KiB 的 write 等待累计 105.815 秒。日志缺少足够的 fd 类型/完整生产消费关联，不能仅据此把等待全归到磁盘或管道。它们也可能是其他同步工作引起的背压。
- Job 统计 user 144.031 秒、kernel 291.094 秒，说明旧诊断运行有显著的内核工作；其中包含日志成本。原生 ReadFile/WriteFile 的提交时间小，不代表异步传输没有等待，也不能把它们当成全部存储成本。
- 旧完整刷新时间、ENOENT 整体耗时、fork 总时间、provider 总时间均不能相加作为“预计节省”。

## 可复查产物

- `artifacts/node-install-20260930/analyze-remaining.py`：本次离线二次解析脚本。
- `io1-analysis/remaining-hotspots.json`：安装阶段返回码、路径类别、原生子调用和 reopen access 汇总。
- `io1-analysis/startup-hotspots.json`：按安装时间窗筛选的启动阶段分布。
- `io1-analysis/fork-hotspots.json`：全阶段 fork 时间、复制字节及次数分布。

以上 JSON 均位于 `artifacts/node-install-20260930/` 下。运行 `python artifacts/node-install-20260930/analyze-remaining.py` 仅重读已有日志并更新这三个分析产物，不启动 guest 或采集新日志。
