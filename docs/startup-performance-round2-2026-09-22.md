# 第二轮大型应用启动优化：2026-09-22

本轮已直接修改执行引擎，候选发行包为 `artifacts/startup2-candidate-dist`。重点消除首次启动的重复解码，以及 FS/GS 跳板生成时不必要的块分析。没有降低完整文件内容哈希、缓存验证、分支入口保护或 fork 映射登记要求。

对照版保留第一轮性能优化，并与候选版包含相同的 unwind 函数入口、间接跳表识别和 GS 兼容性代码。两版来自固定源码快照，仅三个执行模块不同；发行包 SHA-256 比较确认只有执行引擎及引用它的运行时模块不同。系统后台负载与第一轮不同，以下数值应在本轮两列之间比较，不能与第一轮跨时段相减。

| 场景 | 本轮对照中位数 | 优化后中位数 | 耗时变化 | 每版样本数 |
| --- | ---: | ---: | ---: | ---: |
| Java 25，无 AOT 缓存 | 1,504.3 ms | 830.5 ms | 减少 44.8% | 7 |
| Firefox `--version`，无 AOT 缓存 | 10,195.3 ms | 5,681.5 ms | 减少 44.3% | 5 |
| Firefox `--version`，有效 AOT 缓存 | 2,283.5 ms | 1,807.8 ms | 减少 20.8% | 9 |
| Java 25 `-version`，有效 AOT 缓存 | 486.0 ms | 501.2 ms | 增加 3.1%，未证明整体热启动改善 | 11 |
| Chrome `--version`，有效 AOT 缓存 | 1,545.5 ms | 1,483.6 ms | 减少 4.0%，小幅变化 | 7 |

计时包含 worker、init、客体程序运行和退出；退出码必须为 0，且输出必须包含成功标记。热启动预热一次、逐轮交替顺序；关闭分析日志。无 AOT 缓存测试在专用 root 中对**每个构建的每一次启动**分别归档缓存目录，因为两版缓存键相同。未清空 Windows 文件缓存。浏览器数据仅证明版本命令的装载启动，不代表页面、窗口或首帧就绪。

分阶段分析与上述整体计时分开运行。JVM 主库的冷启动解码与补丁阶段从 **639.1 ms 降至 273.8 ms**；有效缓存下的补丁应用中位数从 **18.1 ms 降至 8.6 ms**。Firefox 主库的缓存应用从 **570.5 ms 降至 368.8 ms**。Java 热启动中剩余的补丁耗时已经较小，本轮没有证据证明整次 Java 热启动进一步加速。

实际修改：

- `segment_patch.rs`：控制流遍历仍完整处理所有可达指令及分支，只将需要 FS/GS/系统调用处理的精确指令地址交给补丁阶段。候选地址排序后再处理，保留重叠检测的顺序约束；不再为数百万条普通指令重新构建解码器。
- `instruction_trampoline.rs`：复用 FS 解码时已验证的翻译结果；单条无分支翻译指令使用 `Encoder`，直接接管已有输出缓冲区。需要搬移的后续指令仍使用 `BlockEncoder`，保留分支重定位能力。
- `guest_gs.rs`：为已构造的直线 GS 指令序列直接编码，逐条检查控制流和地址溢出，保留 RIP 相对操作数的编码失败处理。实现依据 [iced-x86 Encoder 文档](https://docs.rs/iced-x86/1.21.0/iced_x86/struct.Encoder.html)；实际编码另与原块编码器逐字节比较验证。

验证结果：

- 执行引擎 **43 通过、0 失败、1 个显式性能基准跳过**；TLS 19、ELF 22、链接器 33 通过，合计 **117 通过**。
- 新增控制流用例验证：立即数中的伪 FS/GS/syscall 不会被选中；syscall 之后的分支目标仍能阻止不安全跳板；汇合路径不会重复处理。
- 新增 FS/GS 编码等价性用例覆盖多个装载地址、RSP/RIP 相对寻址、32 位索引、AH 等高字节寄存器和 GS base 指令。保留真实跳板执行与 syscall 桥测试。
- Java 默认 JIT、线程、UTF-8 文件以及 8 次 `ProcessBuilder` 子进程检查通过；Node 两项综合行为探针通过；真实加载器构造函数/TLS/fork 和原生数学 fork 探针通过。
- Python、pthread 清理、启动接口、分配器介入与 IFUNC 五项探针通过；浏览器 ABI、Firefox GLX 探针通过；worker smoke 清理后进程、对象、事务均为 0。
- Java 14 次独立无缓存启动产生的 AOT 文件 SHA-256 全部相同；损坏缓存能够重建，缓存目录不可用时仍正常启动。
- Firefox 使用单独准备的依赖闭包 root，10 次独立无缓存启动通过，生成的补丁缓存也逐字节一致。未改动正在使用的浏览器 root 缓存。

本轮执行完整工作区测试编译，并运行上述相关测试目标；不将这些结果表述为全仓全部测试通过。第一轮报告中的 X11/VFS 既有失败仍有单独记录。

复核材料位于 `artifacts/`：`startup2-verified-{java,firefox,chrome}/results.json`、`startup2-verified-cache/summary.json` 和 `cache-equivalence.json`、`startup2-verified-firefox-cold/`、`startup2-verified-phase-{java,firefox}`、`startup2-tests/`、`startup2-runtime-probes/`、`startup2-browser-probes/`。`startup2-performance.patch` 是固定对照与候选源码的差异；`startup2-{baseline,candidate}-manifest.json` 和 `startup2-binary-differences.json` 记录源码与二进制身份。

初次恢复旧源码时间戳时，Cargo 曾复用同一二进制；这组数据明确作废，见 `startup2-invalid-comparisons.json`。本文只使用强制重新编译、确认二进制不同后的 `startup2-verified-*` 结果。
