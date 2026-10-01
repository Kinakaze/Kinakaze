# 选定写入路径与当前 main 的完整安装验证

固定 Release 构建 `beb268c57d37632ee6a7ca47c1520d681405579e` 在全新根目录完成 357 包 Node.js/npm 安装，安装阶段耗时 **272.215 秒**。八个阶段全部通过，358 项已安装记录（含种子自带的 kinakaze-base）的包名与版本和此前严格安装一致。debconf 错误列表及 guest stderr 为空，Node/npm、dpkg audit 和完整文件校验通过。180 秒目标尚未达到；严格口径的最佳完整安装为 208.099 秒。

该源码包含已选定的私有写入句柄实时 verity 查询、文件向量 I/O、普通原子创建、compact FD、COW 及 v1.2.0 集成改动。写入打开的独立三轮 ABBA 证据见[组件报告](private-writer-verity-query-2026-10-01.md)；不能把本次完整安装与历史运行的差值单独归因于这一个优化。

| 阶段 | 秒 | 状态 |
| --- | ---: | --- |
| update | 0.791 | passed |
| download | 2.188 | passed |
| plan | 0.508 | passed |
| install | 272.215 | passed |
| node-smoke | 1.350 | passed |
| npm-smoke | 2.005 | passed |
| audit | 0.180 | passed |
| verify | 18.760 | passed |

计时的安装阶段包含维护脚本和 triggers，未禁用 fsync、writeback 或其他持久化操作。下载和安装后的完整文件校验单独计时。种子仍为有效 debconf 的 16,128 文件版本，SHA-256 为 `b4aa01f0ca1e4f11a6156ccebcc2527331e80f4cbe7631cd50e131c235182e83`，未缩减软件包或修改种子支持。

完整原生 Release 构建及导出检查通过。新的集成分发通过普通、transfer、pooled 模式下共 78 项客户机回归，覆盖新写入检查、小文件创建和向量 I/O、PI/futex、Unix datagram、权限、保留 inode 及 fork/exec。冻结的 1951 个源码输入和全部 61 个分发文件在测试后及安装后哈希一致；五个实际执行的基准工具也保持原冻结字节。

本次按用户“直接跑”的指示，结束尚未开始安装的安静起跑等待，随后直接启动测量；此前等待记录保留在证据中。安装期间 270 个一秒采样点中，170 个发现编译进程，270 个发现外部运行实例；主机 CPU 利用率中位数 55.8%。本聊天没有在安装期间编译或运行其他基准。主机并未独占，采样不覆盖所有负载及短命进程。

安装 Job 使用 347.188 CPU 秒，启动 5,391 个原生进程。观察器使用 0.906 CPU 秒。完整 CPU/I/O、进程采样摘要、包版本、哈希及证据路径见[测量记录](measurements/integrated-private-writer-install-2026-10-01.json)。
