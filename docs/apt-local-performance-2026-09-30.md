# APT 本地安装第二轮优化与验证

本轮继续优化本地解包和文件元数据路径。最终产物三次交错对照中，512 个 4 KiB 文件的完整本地 APT 安装中位耗时由 8.590 秒降为 7.665 秒，减少 10.8%；解包减少 13.6%。真实 Node.js/npm 的 357 包安装通过，两版各 23,980 个文件的包内校验和全部匹配。所有安装均保留正常刷盘行为。

## 改动

- 原生挂载的写方式 `open(O_CREAT)` 已经需要写权限，不再为这个已知结论额外执行完整 `stat`。只读方式的 `O_CREAT` 仍查询是否会创建文件，保留只读挂载约束。
- `O_CREAT | O_EXCL` 交给原生独占创建判定是否存在，不再先读取设备/FIFO 类型 EA。已有文件、FIFO 和悬空符号链接均返回 `EEXIST`，不会打开特殊设备或覆盖目标。
- `chown` 使用类型、权限和属主元数据，避免无用的 fs-verity 逻辑长度查询与递归 inode 锁。已经无需权限比较的原生文件 root 请求不再为比较条件执行前置 `stat/fstat`；最终对象仍被打开、固定并在 inode 锁内检查和更新。procfs/cgroup 仍保留存在性检查，避免把不存在路径的 `ENOENT` 变成不支持操作错误。
- 公共 `stat/fstat` 继续通过 fs-verity 的受锁事务取得逻辑长度。跨进程 inode 锁、能力属性清除、特权位清除和非特权调用者检查保持有效。

没有引入路径缓存、后台写盘线程或安装器专用行为，也没有设置 `--force-unsafe-io`、跳过维护脚本或关闭 `fsync/fdatasync`。

比较基线为 `artifacts/bash-job-control-20260930/candidate`，候选为 `artifacts/apt-local-round2-20260930/candidate`。候选包含当前工作区同期的路径解析复用、原生挂载写句柄复用等改动，以下整体收益不能单独归因于上面的某一项。发行文件 SHA-256 记录在每份基准报告中。

## 交错本地基准

使用 `tools/benchmark-dpkg-phases.py`，顺序为旧、新、新、旧、旧、新。每次建立独立包数据库和安装目录，安装 512 个 4 KiB 文件；配置脚本调用 12 次 `/bin/true`。APT 的源为空，直接安装本地 `.deb`，不存在外部网络下载。每次验证全部负载字节、配置标记和 dpkg 状态。

| 阶段 | 旧版中位耗时 | 新版中位耗时 | 耗时减少 |
| --- | ---: | ---: | ---: |
| dpkg 解包 | 5.541 s | 4.786 s | 13.6% |
| dpkg 配置脚本 | 1.121 s | 1.146 s | 耗时增加 2.2% |
| 本地 APT 安装 | 8.590 s | 7.665 s | 10.8% |

三次 APT 样本：旧版 `9.096 / 8.162 / 8.590 s`，新版 `7.226 / 7.665 / 7.975 s`。三次解包样本：旧版 `6.606 / 5.487 / 5.541 s`，新版 `4.750 / 4.793 / 4.786 s`。这些是同一台机器的小样本测量，不代表所有包或硬盘的固定提升比例。此前候选版的初测降幅为 18.6%，原始数据仍保留；本文采用最终产物重新交错测量的较保守结果。

整组测试（I/O 探针、解包、配置和 APT）的子进程数均为 108。中位内核 CPU 时间由 11.047 s 降到 9.016 s，总 CPU 时间由 15.125 s 降到 12.734 s，负载读写字节量相近。

128 文件 I/O 探针的中位耗时：

| 操作 | 旧版 | 新版 |
| --- | ---: | ---: |
| 创建、写入、关闭 | 210.228 ms | 160.380 ms |
| 路径 chown | 75.575 ms | 45.259 ms |
| fchown | 47.206 ms | 24.743 ms |
| fchmod | 33.907 ms | 25.848 ms |
| fsync | 94.731 ms | 114.716 ms |

探针用于区分收益来自哪些操作；刷盘耗时没有被人为削减。

## 原生采样

对前一候选构建 `candidate-r1` 另行运行 4096 文件的解包诊断，27.010 秒完成。47 个快照中，17 个当前指令落在原生 `FlushBuffersFile`，8 个落在 `NtCreateFile`，3 个落在 `NtQueryAttributesFile`。它们提示剩余开销仍包含真实持久化和打开文件；样本数有限，不能据此推算精确 CPU 占比。

采样仅暂停本次测试 Job 内的进程线程，读取寄存器后立即恢复。报告里的栈候选地址不是完整还原的调用栈；本文仅使用当前指令位置。采样运行不纳入上述性能对比。

## 真实 Node.js/npm 安装

两份全新、相同清单生成的独立 Debian root 均验证通过。包源由已有 357 个 Debian `.deb` 构成，先单独完成本地获取，再执行 `apt-get --no-download install nodejs npm`。安装后执行 Node.js 加密功能探针、`npm --version`、`dpkg --audit` 并检查已安装状态，全部通过。

| 记录项 | 旧版 | 最终候选版 |
| --- | ---: | ---: |
| 本地安装墙钟时间，不含获取包 | 513.918 s | 485.746 s |
| dpkg 解包日志跨度 | 495 s | 468 s |
| dpkg 配置日志跨度 | 10 s | 10 s |
| 安装进程树内核 CPU 时间 | 323.391 s | 300.328 s |
| 安装进程树总 CPU 时间 | 465.422 s | 444.703 s |
| 安装进程数 | 5,255 | 5,255 |
| 校验文件数 | 23,980 | 23,980 |
| 校验和不一致 | 0 | 0 |

大型安装各运行一次，单次墙钟耗时减少 5.5%，但旧版安装期间存在构建任务，候选版期间检测到另一个安装基准占用约 0.88 个 CPU 核及共享磁盘。因此这组记录主要证明完整安装和数据正确性，不作为严格的性能因果对照。最终小文件交错复测在这两组大型安装及本轮构建结束后执行。

校验按新安装的 357 个包的 `md5sums` 核对实际落盘内容，无缺失校验清单。包获取仍由 APT 使用本地索引中的 SHA-256 验证；安装后的 MD5 校验用于核对包提供的文件清单，不替代来源验证。两个真实安装的解包/配置日志均保留，逐包耗时仅有 1 秒分辨率。

## 正确性和构建

89 项不同的原生测试通过，另有 5 项既有测试标记为忽略：新增安装路径回归 2 项、overlay 挂载 39 项、fs-verity 38 项、挂载策略 2 项、xattr 8 项。新增回归覆盖独占创建、FIFO/符号链接存在判定、硬链接及重命名后的 fd 属主修改、setuid/setgid 与 `security.capability` 清除、非特权拒绝、无效描述符及不存在的 procfs 路径。增强后的新增回归又单独执行通过。

Release 构建成功，打包 29 个原生模块；`cargo fmt --all -- --check` 和 `git diff --check` 通过。候选目录是独立验证产物，未覆盖正在运行的安装目录。

## 复现与原始记录

```powershell
python tools/benchmark-dpkg-phases.py --root artifacts/goal-systemd-idle/debian-root --dist artifacts/apt-local-round2-20260930/candidate --output artifacts/apt-local-round2-rerun --repeat 3 --files 512 --children 12 --io-probe --apt
```

原始记录集中在 `artifacts/apt-local-round2-20260930/`：

- `final-balanced-results.json`、`final-comparison.json`：最终产物完整对照和中位数；`final-balanced-*`：每次会话记录。无 `final-` 前缀的对应文件为前一候选构建初测。
- `native.json`、`native-extra.json`、`native-final.json`：原生测试输出。
- `profile/report.json`、`profile/samples/`：单独采样诊断。
- `node-baseline/report.json`、`node-candidate/report.json`：真实安装验证。
- `payload-baseline.json`、`payload-candidate.json`：23,980 文件校验结果；`verify-node-payload.py` 为校验脚本。
- `node-package-timings.json`：逐包解包日志统计；`host-activity-candidate.json`：并发负载取样。
- `build-final.log`：最终构建日志，完成后的产物目录为 `candidate`。
