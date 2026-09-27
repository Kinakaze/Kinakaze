# 启动优化：按需构造 provider 元数据（2026-09-26）

本轮接续 [按需绑定优化](startup-demand-binding-2026-09-26.md)，继续消除每次启动对未使用模块的处理。没有提前解析 guest 符号地址、跨进程地址缓存或常驻预热进程。

## 同场对照结果

三个 release 发布目录交替执行，每个场景、每个版本 20 次正式测量及 2 次预热。每次完整创建 supervisor、init、worker 并等待 guest 退出；主表关闭跟踪与分阶段计时。操作系统文件缓存已热，不代表机器冷启动。

| 场景 | 上一轮中位数 | 本轮中位数 | 本轮减少 | 原始基线中位数 | 累计减少 |
| --- | ---: | ---: | ---: | ---: | ---: |
| BusyBox echo | 58.77 ms | 49.97 ms | 15.0% | 77.36 ms | 35.4% |
| Bash builtin echo | 66.70 ms | 57.06 ms | 14.5% | 82.35 ms | 30.7% |
| Python import ssl/sqlite3/json/subprocess | 217.72 ms | 210.25 ms | 3.4% | 234.00 ms | 10.1% |
| Bash 连续启动 10 次 BusyBox | 943.67 ms | 740.86 ms | 21.5% | 1503.84 ms | 50.7% |

240 次正式启动全部通过。各版本均在本轮重新测量，不能把这里的绝对耗时直接与上一份报告不同时间的结果相减。三个版本使用相同的固定源码背景，仅启动优化相关实现不同。

| 场景 | 上一轮 P95 | 本轮 P95 | 上一轮 CPU 中位数 | 本轮 CPU 中位数 |
| --- | ---: | ---: | ---: | ---: |
| BusyBox | 66.15 ms | 54.43 ms | 46.88 ms | 46.88 ms |
| Bash builtin | 72.74 ms | 63.71 ms | 62.50 ms | 46.88 ms |
| Python imports | 225.64 ms | 216.87 ms | 218.75 ms | 203.13 ms |
| spawn10 | 1046.13 ms | 802.85 ms | 968.75 ms | 765.63 ms |

CPU 为整个 Windows Job 的累计时间，计账粒度约 15.625ms；它不是墙钟等待时间。样本量较小，P95 不代表长期尾延迟。spawn10 的进程数仍为 23，本轮收益来自减少每次启动的工作；原始基线的 43→23 是先前控制台优化带来的。

### 宿主负载干扰

首次批次保存在 `paired-final`，BusyBox/Bash/Python 期间宿主 CPU 平均占用分别约 79%、86%、73%，绝对耗时明显膨胀，Python 中位数甚至倒退。该批次不用于主结论。

宿主负载回落后完整重跑三个版本，结果保存在 `paired-settled`，四个场景平均宿主 CPU 占用约 19%–30%。主表全部来自这个完整批次，没有从多个批次挑选最快样本。两个批次的原始日志均保留。

## 改动与瓶颈

此前已经按需加载 provider 和解析符号，但 `ModuleSet::discover` 仍会给全部 29 个模块构造完整 guest 导出元数据，并为对象布局查询加载部分 DLL。

现在普通 worker 使用 `ModuleCatalog`：

1. 启动时检查所有 PE 导出表，包括 Rust 内部导出的 UTF-8、排序、序号、范围和节区属性；检查模块身份、数量及 runtime 生命周期约束。
2. 保留只读文件映射，未使用的文件也不能在校验后被写入、删除或替换。目录中普通 ELF 文件仍交给 ELF linker。
3. 实际 DT_NEEDED/dlopen 命中时才构造该模块的 guest 名称、版本、对象大小和对齐信息，并完整校验。
4. 对象布局查询入口在这次构造中只解析一次；其 DLL 所有权直接移交给绑定层，避免重复打开和路径处理。
5. 首次布局查询加载、provider 绑定及 fork 注册位于同一映射事务内。已有强制生命周期初始化仍保持原顺序。

`ModuleSet::discover` 和 `ModuleImage` 的显式完整检查接口继续保留，用于打包和检查。错误时机有意跟随按需加载：PE 损坏和身份冲突仍在扫描时失败；未使用模块的 guest 声明、版本或布局错误在首次使用它时报告。错误不会触发静默 ELF 回退。

独立分阶段测量，每版 7 次正式样本：

| 阶段 | 上一轮中位数 | 本轮中位数 |
| --- | ---: | ---: |
| providers-discover | 14.34 ms | 4.71 ms |
| providers-shared-load | 0.70 ms | 0.68 ms |
| providers-bind（登记延迟工厂） | 1.69 ms | 1.57 ms |
| providers-total | 17.29 ms | 7.32 ms |
| worker-open-runtime | 4.37 ms | 4.38 ms |
| guest-bootstrap | 5.75 ms | 5.77 ms |
| supervisor-total | 54.52 ms | 44.31 ms |

扫描阶段线程周期中位数从约 3938 万降到 1351 万，说明该阶段实际 CPU 工作减少。部分元数据工作移至后续链接阶段，因此性能结论以主表完整进程计时为准；分阶段计时本身也有扰动。

剩余可见开销主要包括 manager 启动约 8.8ms、worker 创建约 3.5ms、runtime DLL 打开约 4.4ms，以及仍保留的完整 PE 扫描约 4.7ms。这些阶段有包含关系，不能直接相加。Python 的模块导入执行仍占大头，固定启动成本减少对它的百分比收益较小。

## 功能验证

- 相关 Rust 测试 66 项通过、1 项原有忽略。新增测试检查：扫描不执行对象布局加载、保留映射继续禁止写入/删除、未使用的损坏 Rust 导出仍被拒绝、runtime 身份/生命周期错误、按需名称/版本校验，以及显式完整检查仍拒绝坏元数据。
- Release 构建和 29 个模块的打包/导出检查通过；当前工作区 `cargo check --workspace --locked --features kinakaze-v2-runtime/guest-engine` 通过。
- 最终发布包通过 standard ABI、loader entry、environment/fork、daily tools、Python runtime、ForkIo、posix_spawn/PTY 回归。
- 16 个 init-pool 生命周期用例通过，包含真实 Linux Java/JIT/线程/文件/ProcessBuilder。进程池只用于功能验证，没有用于主表提速。
- 真实延迟加载探针验证 RTLD_NOLOAD、dlopen、dlsym、dlvsym、dladdr、引用计数、fork 后同一地址及子进程首次加载新库。BusyBox 仍只实例化 libc/libresolv 两个 provider、解析 361 个 guest 符号。Vulkan 只在请求后解析指定导出，反查没有填充无关符号缓存。
- 未使用的 Vulkan 文件在会话运行期间不能以更新模式打开，会话结束后可以打开。该验证没有写入文件内容。

上述覆盖不等于完整 GUI/GPU/音频业务回归；Vulkan 探针只加载和查询导出，没有执行渲染。

## 文件与复现

本轮只修改 `crates/bridge/src/native.rs`、`crates/loader/src/providers.rs`，新增 `crates/bridge/src/native_catalog_tests.rs`，并更新 bridge 文档。默认 `dist` 没有覆盖。

工件根目录：`artifacts/startup-extreme4-20260926`。

- 最终发布包：`final-dist`；固定构建源码：`source`。
- 最终对照：`paired-settled`；受干扰批次：`paired-final`；汇总：`analysis.json`。
- 分阶段数据：`profile-final`、`profile-summary.json`。
- 功能验证：`regressions-final`、`demand-verification-final`、`tests.log`、`check-workspace.log`。
- 本轮补丁：`changes.patch`；测试源码与工作区 14 个相关文件一致性：`final-source.json`。

复现对照：

```powershell
python artifacts/startup-extreme4-20260926/run_compare.py --candidate final-dist --original --tag paired-repro
python artifacts/startup-extreme4-20260926/verify_deferred.py final
python artifacts/startup-extreme4-20260926/run_regressions.py final
```

源码背景固定是为了隔离本轮效果，工作区其他并行改动没有混入这组发布包。构建目标沿用 `artifacts/startup-extreme3-20260926/build`；该目录是可复用编译产物，不是上一轮不可变发布包。
