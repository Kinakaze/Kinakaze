# 进程身份查询优化与整轮安装验证

180 秒目标尚未达到。这轮完整安装测试没有证明端到端性能提升。

## 修改

`engine/crates/kinakaze-runtime/src/job/liveness.rs` 为每个原生线程保留至多 64 个
不可继承的进程查询句柄。首次打开时验证创建时间，随后仍在每次调用时查询同一个
内核对象的退出状态，避免重复 `OpenProcess`、`GetProcessTimes`、`CloseHandle`。
缓存按 PID 和创建时间匹配，退出后移除，不缓存“仍然存活”的答案。
当前进程的创建时间单独保存；原生 TLS 销毁后的查询退回独立打开。

进程表 sweep、祖先检查及 namespace 收养复用这个检查。没有删除进程行时，
sweep 不再重建两张未变化的索引。PID 复用、退出码 259、僵尸保留、异常父进程退出、
subreaper 收养、等待、TLS 析构期间查询及 fork 映射等 59 项 release 原生测试通过。

## 完整安装

两个固定发行目录均从同一份源码快照构建，使用相同离线种子和 357 个缓存包，
分别安装到全新 rootfs。没有恢复 I/O 日志或启用运行时采样。

| 指标 | 基线 | 候选 |
| --- | ---: | ---: |
| apt 安装 | 368.597 s | 405.078 s |
| Job CPU | 350.641 s | 376.328 s |
| 解包 | 353 s | 389 s |
| 配置 | 8 s | 9 s |
| 原生进程数 | 5,255 | 5,255 |
| Node / npm / dpkg audit / 文件校验 | 全部通过 | 全部通过 |

基线期间有其他会话的安装、编译，候选期间也有另一轮安装及编译。
两次安装不是安静主机上的配对重复实验，不能据此确定回归的归属，也不能宣称整体加速。
候选冻结时同一 runtime 文件还包含其他会话的两项 fork bootstrap 修改：
模块路径缓冲区按需扩容，以及复用已加载 DLL 的 loader 引用。这些变化也在候选二进制中。
后续工作区的 provider 目录共享、文件元数据和读取缓存改动不在这一候选快照内。

本轮整包安装仍使用修复 debconf 之前的种子，预配置失败警告也被保留在原始日志。
后续 debconf 修复及其独立验收见 `debconf-preconfiguration-2026-09-30.md`。

## 交替微基准

`tools/benchmark-process-identity.py` 在同一完整 rootfs 上交替运行两个固定发行目录，
一轮预热、三轮计时，每次检查进程身份和子进程返回状态。

第一组每次 40,000 次本进程查询、64 次 fork→exec→wait：

- 本进程查询中位数：42.756 → 35.255 ms，降低 17.5%。
- 64 次 fork→exec→wait 中位数：4,898.468 → 4,883.653 ms，差异约 0.3%，处于波动范围。

第二组增加由管道保持存活的子进程，对其执行 40,000 次身份查询，检查跨进程句柄复用；
每轮另执行 8 次 fork→exec→wait。子进程查询中位数为 111.799 → 51.467 ms，降低 54.0%。
结果保存在 `identity-descendants/report.json`。
这些微基准不等于完整安装收益。

## 可复查产物

- `artifacts/node-install-20260930/p180-baseline/report.json`
- `artifacts/node-install-20260930/p180-liveness/report.json`
- `artifacts/install-180-20260930/identity-benchmark/report.json`
- `artifacts/install-180-20260930/identity-descendants/report.json`
- `artifacts/install-180-20260930/native-tests.json`
- `artifacts/install-180-20260930/baseline-source.json`
- `artifacts/install-180-20260930/candidate-source.json`
- `artifacts/install-180-20260930/source-changes.json`

性能候选是在最终补入 TLS 析构回退前构建的；最终回退及其回归测试已在 release 原生测试中验证。
源码快照后续只补入了这项修正，历史候选的二进制与源码哈希清单保持保留。
