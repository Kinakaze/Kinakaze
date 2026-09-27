# 启动优化：按需加载和符号绑定（2026-09-26）

这轮在已有启动优化之上，把 provider 的实际加载和 guest 符号地址解析移到首次使用时。没有提前绑定、地址持久缓存或预热进程池。现有强制初始化回调仍按原顺序执行。

## 测量

Windows 本机 release 构建，固定同一份源码背景，仅替换本轮相关文件。每种配置、每个场景 20 次正式测量、2 次预热，配置交替执行；每次重新启动 supervisor、init、worker。操作系统文件缓存已热，不代表机器冷启动。主表关闭跟踪和分阶段计时。

| 场景 | 原始基线中位数 | 上一轮中位数 | 本轮中位数 | 较上一轮减少 | 累计减少 |
| --- | ---: | ---: | ---: | ---: | ---: |
| BusyBox echo | 66.17 ms | 60.61 ms | 50.81 ms | 16.2% | 23.2% |
| Bash builtin echo | 75.52 ms | 69.22 ms | 58.80 ms | 15.1% | 22.1% |
| Python import ssl/sqlite3/json/subprocess | 215.93 ms | 209.98 ms | 199.90 ms | 4.8% | 7.4% |
| Bash 连续启动 10 次 BusyBox | 1387.25 ms | 1067.90 ms | 866.85 ms | 18.8% | 37.5% |

240 次正式启动全部通过。BusyBox、Bash、Python、spawn10 的本轮 P95 分别为 53.98、65.08、208.35、907.31 ms；样本量只有 20，不作长期尾延迟保证。测量期间宿主 CPU 平均占用约 12%–19.5%。

分阶段独立测量显示，provider 总准备时间约 29.36→17.79 ms，剩余发现和元数据扫描约 14.93 ms。延迟绑定把部分工作移到链接阶段，因此不能单凭 providers-bind 阶段的下降声称端到端收益；以上完整进程计时才是结果。

## 行为与验证

- 普通 provider 在 DT_NEEDED/dlopen 实际命中时加载，符号只在实际重定位、dlsym/dlvsym 或 COPY 请求时解析并缓存。BusyBox 从原先处理 29 个模块、5800 个导出，变为实际实例化 libc/libresolv 两个 provider、绑定 361 个 guest 符号。这不等于物理上只加载两个 DLL，宿主依赖和布局查询仍可能加载其他 DLL。
- RTLD_NOLOAD 只查询身份，不触发延迟工厂；dladdr 的反查不会缓存沿途无关符号。真实 Vulkan 导出查询探针验证了 dlopen 后、dlsym 前没有 Vulkan guest 符号绑定。
- PE 名称、排序、范围、节区和序号检查仍保留，包括 Rust 内部导出。已检查的文件保持只读映射，延迟使用之前不能写入、删除或替换；会话退出后释放。
- 工厂仅执行一次，失败被缓存并正确向上传递；版本不匹配不会触发地址查询。fork 在子进程重建 provider 状态，验证原有地址及 fork 后首次 dlopen。
- 本轮相关 Rust 测试 63 项通过、1 项原有忽略；release 构建和当前工作区 cargo check 通过。
- 真实 guest 回归覆盖 standard ABI、loader entry、环境/fork、daily tools、Python runtime、共享文件偏移、posix_spawn/PTY，以及 16 个 init-pool 生命周期用例（含 Linux Java/JIT/线程/文件/ProcessBuilder）。另增加按需绑定探针。Vulkan 只验证加载和符号查询，没有执行渲染。

## 复现与工件

- 固定源码和最终发布目录：`artifacts/startup-extreme3-20260926/source`、`final2-dist`。
- 三组交替测量：`python artifacts/startup-extreme3-20260926/run_final.py`，原始数据在 `paired-final`；`analysis.json` 保存汇总。
- 功能日志：`regressions-final2`、`tests-final.log`、`check-final.log`；不提前绑定与文件锁验证：`demand-verification`。
- `final-source.json` 记录测试源码与当前改动文件的一致性。基准使用固定背景，当前工作区其他并行修改未混入这组发布包。

此报告对应按需绑定这轮；后续扫描优化单独对照这里的 `final2-dist`，不覆盖本组结果。
