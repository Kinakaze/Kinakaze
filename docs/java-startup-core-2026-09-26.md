# Java 启动瓶颈：从 1.92× 降至 1.43×，尚未达到 1×

本轮保留五项优化。最终同批测试中，启动到 Java main 的中位数由 **85.711 ms 降至 63.649 ms，减少 25.74%**；本机为 **44.586 ms**。仍有约 **19.063 ms** 差距，不能宣称整体达到原生速度。

## 最终对照及口径

数据：`artifacts/java-core-20260926/paired-delivery/results.json`。每组 3 次预热、60 次正式运行，轮换并反转测量顺序，180 次正式运行全部通过。测量期间没有同时运行本任务的编译或回归；宿主 CPU 忙碌比例 17.92%。

| 启动方式 | main 中位数 | main P95 | 退出中位数 | main 相对本机 |
|---|---:|---:|---:|---:|
| 本机 Windows Java | 44.586 ms | 47.548 ms | 61.109 ms | 1.00× |
| 上轮 session 版本 | 85.711 ms | 91.760 ms | 103.118 ms | 1.92× |
| 本轮最终版本 | 63.649 ms | 67.539 ms | 77.718 ms | **1.43×** |

测量外部 `init launch --wait` 客户端启动至宿主观察到 main 标记，以及客户端退出。一个 Linux 环境已有一个长期存活的 init，池中有一个未使用的通用 READY worker；每次执行使用新 worker、新 JVM。READY 前不映射应用 ELF、不执行应用初始化、不解析来宾符号。OS 缓存及该会话经实际请求填充的原始文件快照缓存已热。init 创建、通用 worker 准备不计入这项会话启动延迟。

本机使用已安装的 Oracle JDK 23.0.1，来宾使用 Temurin Linux 25.0.4.1；运行同一份 `--release 17` 字节码，摘要记录在结果中。两者版本不同，不能把差距全部归因于兼容层。没有修改 Java 默认堆、CDS、JIT 或功能配置来改变比较结果。

此前同口径 60 轮 `paired-final` 为 83.692→62.329 ms，本机 43.395 ms，比例 1.436×。两批均支持约 1.43× 的结论；不跨批次拼接绝对耗时。

## 保留的改动

1. **大型 ELF 快照直接并行读取。** 至少 16 MiB 的读取使用最多四个线程，写入互不重叠的页对齐范围；替换此前并行触页后串行读取的方案。每个请求有独立 OVERLAPPED 和事件，退出前取消并回收未完成 I/O；逻辑 EOF、verity 校验、inode 锁和 fork 映射事务仍生效。
2. **TLS 指令编码复用临时空间。** 在同一个未发布代码段中复用 iced 编码器、寄存器分析器和字节缓冲区，减少逐站点分配。缓冲区不包含跨进程复用的符号绑定；补丁仍在本次实际加载时生成和发布。
3. **拆分尚未 fork 的匿名映射时复用 section。** 确认原始 allocation protection 为共享可写映射后，复用已有 backing，消除 JVM CDS 小范围覆盖导致的整段复制。真正的 fork COW 映射保留原复制路径，私有脏页不丢失。
4. **并行创建大型匿名映射的独立视图。** 至少 64 MiB 时，保留 16 MiB 分块，最多四个线程创建和保护独立视图；完成全部 join 后再发布注册信息。占位范围、失败回滚及 fork 同步仍由原事务控制。
5. **init 按需持有原始只读快照及完整摘要。** 缓存 8–64 MiB 文件，LRU 总预算 256 MiB。worker 的真实文件请求触发填充；init 只保存原始字节和完整 BLAKE3，不解析 ELF、执行 JVM 或缓存已绑定符号。

第五项复用由文件身份、固定 inode 和 Windows read oplock 保证；写入触发失效，恢复旧 mtime 不能绕过失效。init 校验 RPC 对端 PID/出生时间及角色，复制其真实文件描述符，拒绝可写源描述符。传出 section 只有查询、读取、执行能力，worker 的写入为私有 COW。存在 verity 元数据时走原有校验读取路径。

租约由 session 内惰性创建的长期线程持有。连接线程结束会取消其发出的未完成 Windows I/O，因此租约不能归连接线程所有。init 同时保留只读视图，避免首次填充后立即 unmap 大量页面；这些视图共享预算内 backing，没有锁定物理页。内部协议升为 V2；原有 V1 控制/启动客户端仍可连接新 init。

## 剩余瓶颈

- **完整启动仍为 1.43×。** 六次诊断运行的 JVM `Create VM` 阶段中位数：本机 28.137 ms、来宾 27.650 ms。这个阶段已接近本机，但不包含启动器及全部 ELF 加载，不能替代完整启动结论。
- **ELF 加载和代码准备仍有成本。** 热缓存 libjvm 快照获取约 0.14–0.20 ms，摘要读取为数微秒；代码补丁应用仍约 3 ms，并有校验、映射归一化、依赖解析及重定位。分段日志有额外成本，阶段相互包含，不能相加充当总耗时。
- **进入初始来宾代码前仍约 8–9 ms。** 轻量时间线中工作目录初始化约 2 ms、解码器首次初始化约 0.45–0.54 ms，另有初始依赖图和链接。删掉一次重复目录检查未改善整批结果，因此保留原行为。
- **Java 内部派生进程仍约 2×。** `internal-tree-cache-resident-dist/results.json`：Java 父进程用 ProcessBuilder 创建新 JVM，30 次正式运行，本机 main 中位数 50.051 ms、来宾 99.924 ms，比例 1.996×。与外部 READY worker 启动口径不同。当前 descriptor handoff、jspawnhelper/exec 及新 Windows worker 初始化仍在路径上；本轮没有把外部池的成绩套用到内部派生。

首次请求也单独测试：`first-launch-cache-resident-dist/results.json` 每次新建 session，使 init 快照缓存为空，OS 缓存仍热。24 次全部成功，来宾 main 中位数 69.368 ms、P95 77.739 ms，本机 44.500 ms；未启用 session 快照缓存的四项优化版本为 68.608 ms。缓存收益主要出现在后续实际请求。

这批最大值为 1035.224 ms，出现在第 0 次；另一批修复后版本的 1108.063 ms 最大值同样只在第 0 次。之后 32 次带分段日志的全新 session 没有复现。保留全部离群值，目前无法确定首次使用构建产物时的额外等待来自哪里，不归因于杀毒、调度或 oplock。

## 撤回的实验与修复

- 并行准备补丁 COW 页：40 轮仅改善约 0.22 ms，主要把串行成本转移到辅助线程，未保留。
- TLS 简单加载复用目标寄存器及直接返回跳转：指令执行测试通过，但 60 轮整体中位数没有改善，未保留。
- 删除 chdir 的重复 stat：60 轮无整体收益，未保留。
- 早期缓存版本用同步 `FileExt::seek_read` 读取异步句柄，在真实 pending I/O 下曾使 init 中止。最终代码改为显式 OVERLAPPED，新增强制 pending 及取消回收测试；修复后两批共 56 次空缓存启动均成功。早期 `cache-ready-dist` 等中间产物不作为交付版本。

## 验证与产物

- 最终 init、manager、protocol 测试：75 通过、1 项原有忽略。包括缓存内容与权限、租约生命周期、完整摘要、异步取消、角色授权、fork/exec 状态和协议兼容。
- 相关 engine、immutable、image I/O、link 测试：92 通过、2 项原有忽略；匿名映射相关 fdio 测试：87 通过、1 项原有忽略。
- 最终产物通过 8 组 ABI、加载入口、环境/fork、日常工具、Python、共享 fork I/O、PTY/进程启动和 init 池回归；池回归包含 Java ProcessBuilder。
- 最终产物通过 12 个跨 worker 快照场景：原 inode 写入并恢复时间戳、命中、重命名/unlink、路径替换、truncate、fork、后续启用 verity，以及保留描述符的宿主篡改被拒绝。
- 最终会话审计通过 6 场景：READY 前无应用 ELF/来宾绑定、外部启动的 parent/cwd/env/exit、无效 parent 回收、内部派生保持单 init、客户端断开不杀应用、拒绝过期 session。延迟 dlopen/dlsym、版本和 COPY 回归通过。
- 工作区 `cargo check --workspace --locked --features kinakaze-v2-runtime/guest-engine` 通过。

最终产物为 `artifacts/java-core-20260926/final-dist/`，默认 `dist/` 未覆盖。固定源码在 `source/`，29 个相关文件及摘要在 `final-source.json`；工作区并发新增的 libc program-break/temporary 功能没有纳入该固定性能对照，也没有被覆盖。行尾差异单独记录。本轮没有提交或暂存其他任务的改动。

复测命令：

```powershell
python artifacts/java-session-20260926/compare_sessions.py --repeat 60 --warmup 3 `
  --dist F:/crysoacu2/artifacts/java-core-20260926/final-dist `
  --previous F:/crysoacu2/artifacts/java-session-20260926/final-dist `
  --tag F:/crysoacu2/artifacts/java-core-20260926/recheck

python artifacts/java-core-20260926/run_regressions.py final
python artifacts/java-core-20260926/audit_cache.py final-dist
python artifacts/java-core-20260926/verify_deferred.py final
python artifacts/java-core-20260926/run_cache_mutation.py final
```
