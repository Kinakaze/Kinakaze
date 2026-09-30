# APT 热点优化运行验收（2026-09-30）

本次对 `candidate-hotpaths-test` 冻结发行目录进行了实际运行验证。功能与资源检查通过，但完整 Node/npm 安装仍需 **433.935 秒**，没有证据说明已达到快速安装目标。

所有产物均在 `artifacts/node-install-20260930/`。测试 root 为通过既有 seed 清单新建的 `hotpaths-test-root`；没有复用已安装 Node/npm 的 root，也没有用硬链接复制可写文件。构建日志为 `build-hotpaths-test.log`，关键源码哈希为 `source-hotpaths-test.json`，构建期间和安装期间检查未发现这些文件变化。发行文件 SHA-256 保存在各运行报告中。

## 回归结果

| 检查 | 结果 | 证据 |
| --- | --- | --- |
| PE 导出索引、账户库、VFS、fs-verity、ELF 映射等原生回归 | 94 项通过；2 项既有诊断/基准默认忽略 | `native-hotpaths-test.json` |
| init 共享镜像、lease、快照隔离与哈希 | 8 项通过 | `cache-hotpaths-test.log` |
| 默认双 worker 的预热、补充、崩溃替换与退出清理 | 15 项检查通过 | `pool-hotpaths-test/results.json` |
| NativeLookup、SparseForkStack、AllocationGrowth | 全部通过 | `guest-hotpaths-test/` |
| 账户数据库实际 ABI 与运行中资源检查 | 通过 | `account-hotpaths-test/results.json` |
| session 正常退出及超时回收 | 8 次正常循环加 1 次超时检查通过 | `resources-hotpaths-test/results.json` |

账户专项在进程自己的 chroot 中检查了可读空表的权威性、坏记录跳过、大 UID/GID、按路径替换后的更新、ERANGE 后重试同一条记录、fork 后父子继续枚举和重复 EOF。随后在同一进程完成 17,408 次循环内按键查询，穿插完整 passwd/group 枚举。预热及两轮观察的句柄均为 **277**、线程均为 **6**、私有提交均为 **178,806,784 字节（170.52 MiB）**。

session 回收检查保留已经关闭的 Python pool 对象，避免依赖 GC 掩盖未释放资源。每轮关闭后的输出缓冲、打开的流、输出线程、采样对象、Job 句柄和 watchdog 均已清理；宿主测试控制进程句柄保持 166、线程保持 4，私有提交从预热后的 12,128,256 字节至最终 13,639,680 字节，处于测试上限内。这些结果覆盖测试所执行的生命周期，不代表排除了所有可能的泄漏。

## 完整安装

使用原有本地 `.deb` 仓库，先下载再执行 `apt-get --no-download install nodejs npm`，不启动全量 I/O 诊断或堆栈采样。维护脚本、触发器及持久化操作均保留。报告为 `install-hotpaths-test/report.json`。

| 阶段 | 时间 |
| --- | ---: |
| 本地下载 | 3.174 秒 |
| 完整安装 | **433.935 秒** |
| 其中 dpkg 解包 | 415 秒 |
| 其中 dpkg 配置 | 10 秒 |
| Node JavaScript/crypto 检查 | 0.843 秒 |
| npm 版本检查 | 1.357 秒 |
| dpkg --audit | 0.162 秒 |
| dpkg --verify | 24.142 秒 |

解包/配置时间来自 dpkg 的秒级日志，完整安装还有其他阶段，不能要求分项精确相加。357 个新增包全部配置完成，已安装版本映射与历史 r8 报告一致。Node 为 18.20.4，npm 为 9.2.0；audit 和 verify 输出均为空。

安装记录 5,255 次原生进程生命周期，Job CPU 累计 420.094 秒（user 147.297 秒、kernel 272.797 秒）；整个验收 session 的峰值 Job 提交为 769,683,456 字节（约 734.03 MiB）。session 结束后，按本次发行目录筛选，剩余 init/worker 等进程数为 **0**。紧凑结果见 `hotpaths-test-summary.json`。

历史 seed 中的 virtual base 声明提供 debconf，却没有独立 debconf 状态记录，因此保留了历史基线也存在的 apt-extracttemplates/debconf 版本警告。本轮最终 apt 退出、包配置状态及内容校验均通过，未修改 seed 或跳过预配置钩子来消除提示。

## 性能判断范围

本轮为尽快完成验收，与剩余原生测试编译并行；机器上也有其他构建和用户 session。相关现场记录为 `install-hotpaths-host-context.json`。因此 433.935 秒是本次功能验收的实际耗时，**不是隔离的 A/B 性能成绩**。

它低于早先约 512 秒的诊断运行，但高于历史无诊断 r8 的 359.73–429.33 秒范围。环境负载、诊断配置及预热设置不同，不能据此宣称最新修改已经产生稳定提速，也不能单凭此轮认定某个改动导致退化。本轮按限定范围完成验收后停止，没有追加安装重复轮次。
