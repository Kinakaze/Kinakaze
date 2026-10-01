# 私有写入打开的合并元数据查询

候选未满足预先确定的全部选择条件，不合入生产；主分支继续保留上一轮选定路径。 三项检查分别为：各轮写入一致受益未通过，读取退步限制未通过，整个探针 Job CPU 降低通过。

## 实现与语义

候选用一次命名 EA 查询读取同一次私有打开的 Linux inode 和 verity 记录。常规文件分类仍先检查 inode；verity 解析错误和恢复结果延后到原有挂载、权限检查之后处理。私有 native writer 在此期间保持存活，排除新的 verity enable；PREPARING/BUILT 仍在 inode 锁下重新读取并执行原有持久化恢复。没有跨打开缓存元数据。普通读取也使用公共打开实现，因此单独检查其性能。

## 完整对照

控制分发是 `beb268c57d37632ee6a7ca47c1520d681405579e` 的正常 Release 构建，61 个文件按原字节复制；候选仅改动三个生产文件和一个回归文件，再进行正常 Release 构建。两份分发均通过 29 模块、5,973 项导出检查。全部源码差异和两份分发的前后哈希已核对。

预先确定三轮 A/B/B/A，共 12 次会话，每次在计时前重建 512 个文件。六个写入打开循环和四个只读打开循环各执行 8,192 次，最后检查全部文件内容。没有删除样本；主机未独占，本聊天在对照期间没有编译或执行另一项基准。

| 循环 | 控制均值 ms | 候选均值 ms | 降低 |
| --- | ---: | ---: | ---: |
| absolute_repeated | 1180.896 | 1251.501 | -5.98% |
| absolute_varied | 1230.701 | 1255.106 | -1.98% |
| absolute_varied_write | 2369.215 | 2068.922 | 12.67% |
| relative_repeated | 1562.532 | 1485.758 | 4.91% |
| relative_varied | 1531.590 | 1447.696 | 5.48% |
| relative_varied_write | 2467.288 | 2536.735 | -2.81% |
| read_absolute_repeated | 492.478 | 553.426 | -12.38% |
| read_absolute_varied | 508.265 | 597.063 | -17.47% |
| read_relative_repeated | 862.299 | 751.676 | 12.83% |
| read_relative_varied | 832.457 | 748.056 | 10.14% |

整个探针 Job CPU 均值从 13.568 秒到 13.320 秒，包含启动及文件准备。

| ABBA 轮次 | 循环 | 控制 ms | 候选 ms | 降低 |
| --- | --- | ---: | ---: | ---: |
| 1 | absolute_repeated | 1083.784 | 1035.719 | 4.43% |
| 1 | absolute_varied | 1077.149 | 1055.409 | 2.02% |
| 1 | absolute_varied_write | 1890.933 | 1771.938 | 6.29% |
| 1 | relative_repeated | 1364.418 | 1277.907 | 6.34% |
| 1 | relative_varied | 1304.847 | 1249.590 | 4.23% |
| 1 | relative_varied_write | 2141.989 | 2163.505 | -1.00% |
| 1 | read_absolute_repeated | 466.281 | 486.063 | -4.24% |
| 1 | read_absolute_varied | 484.149 | 500.207 | -3.32% |
| 1 | read_relative_repeated | 682.384 | 649.457 | 4.83% |
| 1 | read_relative_varied | 673.091 | 686.124 | -1.94% |
| 2 | absolute_repeated | 1134.330 | 1354.565 | -19.42% |
| 2 | absolute_varied | 1150.198 | 1333.036 | -15.90% |
| 2 | absolute_varied_write | 2046.678 | 2352.120 | -14.92% |
| 2 | relative_repeated | 1330.208 | 1805.071 | -35.70% |
| 2 | relative_varied | 1350.363 | 1656.167 | -22.65% |
| 2 | relative_varied_write | 2440.391 | 2516.852 | -3.13% |
| 2 | read_absolute_repeated | 483.545 | 574.514 | -18.81% |
| 2 | read_absolute_varied | 504.077 | 576.135 | -14.30% |
| 2 | read_relative_repeated | 1169.881 | 707.492 | 39.52% |
| 2 | read_relative_varied | 1032.796 | 724.089 | 29.89% |
| 3 | absolute_repeated | 1324.572 | 1364.218 | -2.99% |
| 3 | absolute_varied | 1464.757 | 1376.874 | 6.00% |
| 3 | absolute_varied_write | 3170.034 | 2082.709 | 34.30% |
| 3 | relative_repeated | 1992.970 | 1374.296 | 31.04% |
| 3 | relative_varied | 1939.560 | 1437.330 | 25.89% |
| 3 | relative_varied_write | 2819.484 | 2929.849 | -3.91% |
| 3 | read_absolute_repeated | 527.608 | 599.699 | -13.66% |
| 3 | read_absolute_varied | 536.570 | 714.847 | -33.23% |
| 3 | read_relative_repeated | 734.632 | 898.079 | -22.25% |
| 3 | read_relative_varied | 791.483 | 833.956 | -5.37% |

## 验证

116 项原生回归通过，另有 4 项已声明忽略的辅助测试/基准；普通、transfer、pooled 三种模式的 78 项客户机检查全部通过。新增回归覆盖实时 inode 变化、损坏或 ENABLED verity 记录、延期返回错误，以及名称替换、删除后仍恢复原 inode。既有用例继续验证私有 writer 排除 enable、权限、挂载、fork/exec 和描述符别名。

这一轮只判断组件路线；没有用局部循环推算完整安装时间。此前集成严格安装为 [272.215 秒](integrated-private-writer-install-2026-10-01.md)，严格口径最佳完整安装仍为 208.099 秒，180 秒目标尚未达到。

[测量记录](measurements/compound-writer-query-2026-10-01.json) 保存选择条件、全部 12 行原始结果、双份分发哈希和证据摘要；原始文件位于 `artifacts/goal-install-180-20260930/r18-*`。
