# VFS 小文件创建与离线 npm 安装：2026-10-01

本轮减少普通 `O_CREAT` 新文件的路径解析和元数据重开。完整离线 npm 安装的耗时中位数从 **41.543 s 降至 40.102 s（3.5%）**，宿主作业 CPU 时间降低 **23.0%**，宿主查询等非读写操作计数降低 **42.6%**。该结果来自 256 个本地 tarball 包，每次安装校验 8,704 个文件；包含 npm 的正常 postinstall 执行。

## 实现与选择

- 普通原生路径的可写 `O_CREAT` 现在复用已经验证的父目录固定与 NT `FILE_CREATE` 路径，适用于 npm/Node 常用的 `O_CREAT|O_TRUNC`。模式、umask 和实时 SGID 归属作为初始 EA 与文件一同发布。
- 原子创建碰到已有对象后回到普通打开流程，保留已有文件模式、截断前权限及 fs-verity 检查、符号链接和目录语义。
- 普通路径快速创建仍检查当前能力、命名空间、挂载与原生父目录，复杂路径和保留 dirfd 仍使用完整解析。
- 普通原生文件的通用 `O_CREAT` 流程也在创建前检查父目录权限，并以初始 EA 创建。拒绝目录写权限时，已有叶子仍可按自己的权限打开；该分支使用 `OPEN_EXISTING`，避免并发 unlink 导致未经授权的新建。
- 新比较开关 `KINAKAZE_TEST_NATIVE_CREATE_NONEXCL` 仅存在于测试构建，生产构建中未检出该字符串或 benchmark 标记。现有原生创建诊断总开关保持原有默认开启设置。

## 原生组件对照

同一冻结 release 测试二进制，先预热一组，再交替执行七组；每次覆盖 896 个文件。计时覆盖 public VFS 的创建、写入、关闭，目录准备与内容/模式校验在计时之外。控制组是本轮已改为原子初始化的通用路径，候选组启用普通路径快速创建，因此这里比较的是快速创建本身的增益。

| 文件负载 | 控制组 µs/文件 | 候选组 µs/文件 | 耗时降低 |
| --- | ---: | ---: | ---: |
| 512 B | 449.916 | 348.659 | 22.5% |
| 4 KiB | 447.696 | 349.750 | 21.9% |
| 五层嵌套 node_modules，4 KiB | 470.572 | 383.989 | 18.4% |
| 64 KiB | 532.740 | 393.878 | 26.1% |

## 真实 npm install 对照

控制构建为 `4720b461f58e8cc2f0056babf271d4497ab1427b`，候选构建为相同源码基线加本轮改动；两边均来自完整 Cargo workspace release 构建，分别冻结全部 38 项分发构件。没有混用不同修订的 DLL。

256 个确定性本地 `.tgz` 包，每包 32 个 4 KiB 文件及 package.json/index.js。每轮项目和 npm 缓存均为新目录，执行 `npm install --offline --no-audit --no-fund`，没有禁用 lifecycle。根项目 postinstall 写入标记，安装结束后逐文件核对 SHA-256、检查 lockfile 包数量及脚本标记。先预热一组，再交替五组；报告完整保留预热数据。

| 正式轮次 | 控制组 s | 候选组 s |
| --- | ---: | ---: |
| 1 | 45.362 | 41.612 |
| 2 | 43.742 | 42.273 |
| 3 | 41.543 | 40.102 |
| 4 | 36.903 | 38.589 |
| 5 | 37.641 | 38.519 |

| 五轮中位数 | 控制组 | 候选组 | 降低 |
| --- | ---: | ---: | ---: |
| 完整安装耗时 | 41.543 s | 40.102 s | 3.5% |
| 宿主作业 CPU 时间 | 50.078 s | 38.562 s | 23.0% |
| 宿主非读写操作计数 | 1,761,426 | 1,010,678 | 42.6% |

耗时在共用宿主上有波动，后两组候选耗时高于控制组；不能把组件提速比例直接作为完整 npm 安装收益。此次结论限于该离线合成依赖图和小文件规模，未测量网络下载。

## 验证与复现

- 同一冻结原生构建，开启和关闭新快速路径分别 **900 passed / 0 failed / 20 ignored**。
- 六个 guest 回归通过：普通创建、排他创建、排他权限/挂载、已有文件写入、相对路径写入、原生权限。新回归覆盖原子并发创建、实时 SGID、umask、已有叶子截断/模式、符号链接、保留 dirfd 及降权目录拒绝，确认拒绝后未留下新文件。
- 两种完整生产构建各通过六次 npm 安装，共校验 104,448 个包内文件，十二次 postinstall 标记均正确。

```powershell
python tools/benchmark-futex-queues.py --binary <冻结的 kinakaze_vfs 测试 exe> --case smallfile-create --rounds 7 --output <native-report.json>
python tools/test-native-smallfile-create-guest.py --root <已安装 Node/npm 的 guest root> --dist <生产分发> --output <新输出目录>
python tools/benchmark-npm-smallfiles.py --root <已安装 Node/npm 的 guest root> --control <冻结控制分发> --candidate <冻结候选分发> --output <新输出目录> --rounds 5 --packages 256 --files 32 --size 4096
```

原始构建、冻结分发、逐轮报告和 guest 日志保存在 `artifacts/vfs-smallfile-create-20261001/`。版本化摘要及报告摘要哈希见 [测量记录](measurements/vfs-smallfile-create-2026-10-01.json)。

## 与最新 main 的集成验证

集成源码图 `ae46ed7` 合入 main `bc98351`，保留双方的 benchmark 入口。完整 workspace release 构建通过并冻结全部 38 项构件；在该构建上再次通过六个 guest 回归。普通创建回归新增新建文件的 `readv/writev`、dup 共享偏移、rename/unlink 后继续读写的组合检查，均通过。

集成构建还使用相同 256 个 tarball 完成一次独立 npm 安装，8,704 文件哈希、256 个 lockfile 条目及 postinstall 标记全部正确。该次仅验证兼容性，不计入前面的新旧性能对照；前述五轮测量仍明确属于 `4720b46` 加本轮改动的冻结队列。集成生产映像未检出新创建或向量 I/O 的测试开关字符串。
