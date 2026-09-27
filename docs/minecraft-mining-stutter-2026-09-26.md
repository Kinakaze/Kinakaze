# Minecraft 挖掘时掉帧分析（2026-09-26）

## 结论与范围

用户明确描述为挖掘方块时 FPS 下降。当前证据最支持先检查 **区块构建器的半阻塞模式**：它把玩家修改造成的区块重建放进当前渲染调用，能够直接造成帧时间尖峰。代码中的触发机制已经核实，但没有采集挖掘瞬间的帧时间或调用栈，尚不能量化耗时，也不能认定这是唯一原因。

本次只读日志、配置与本地客户端字节码，没有启动 Minecraft、注入输入、修改游戏设置或改动兼容层实现。

可用记录来自 `artifacts/promo-v1/guest-root/minecraft/v2-demo` 的上次游玩。新完整本地模式目录 `minecraft/v2-game` 在分析时还没有日志或选项文件；因此以下配置不能直接当作完整模式已运行后的实测状态。

## 挖掘触发的渲染阻塞

旧目录 `options.txt:18` 为 `prioritizeChunkUpdates:1`。本地客户端的枚举与语言资源一致：

| 值 | 游戏界面 | 构建方式 |
| --- | --- | --- |
| 0 | 线程化 / Threaded | 将重建任务交给异步调度 |
| 1 | 半阻塞 / Semi Blocking | 玩家修改的区块同步构建 |
| 2 | 全阻塞 / Fully Blocking | 附近或被玩家修改的区块同步构建 |

对已安装的 Minecraft 26.2 客户端进行了静态检查：

1. `LevelRenderer.render` 在渲染流程中调用 `compileSections`。
2. `compileSections` 在模式为 `PLAYER_AFFECTED` 且 `playerChanged()` 为真时调用 `RenderSection.compileSync`，否则调用 `compileAsync`。
3. `compileSync` 直接调用任务的 `doTask(fixedBuffers)`；`compileAsync` 则将任务交给调度器。

因此破坏方块后，即使没有进入新区域，也可能把区块几何重建工作加入当前帧。场景复杂程度、光照与后续 GPU 上传等待可能影响尖峰大小，需要采样区分。

静态证据位于 `artifacts/mouse-debug/mining-analysis/`：`PrioritizeChunkUpdates.txt`、`LevelRenderer.txt`、`SectionRenderDispatcher$RenderSection.txt`。本机 javap 为 Java 23，不接受客户端 Java 25 的 class 版本号；为读取反汇编，仅在该分析目录的三个 class 副本中把版本头从 69 改为 67，未修改指令或原始 JAR，未运行副本。选项名称及提示另与原始游戏语言资源交叉核对。

## 其他证据及其限制

- `latest.log:28` 显示实际图形设备为 Intel Iris Xe；`options.txt` 中渲染距离为 16、模拟距离为 12，开启垂直同步。该配置可能减少挖掘更新时的性能余量，但没有 GPU 耗时数据，不能单凭显卡型号认定 GPU 已饱和。
- 日志记录进入世界后多次扩大区块 UBO 容量，从 2 增长到 1024；这说明存在渲染资源扩容，未记录每次耗时，且没有挖掘事件时间戳，不能认定每次扩容都对应用户感到的卡顿。
- `latest.log:239` 记录世界服务器落后 2032ms / 40 ticks。这证实那次会话存在逻辑线程延迟，但它不是“渲染线程阻塞 2032ms”的测量，也不能替代挖掘掉帧证据。
- 启动脚本默认 `-Xmx2G`，没有 GC / safepoint 日志。GC 可以列为待验证因素，当前不能称为已确认的垃圾回收卡顿，更不能用兼容层原生 malloc 基准直接代表 Java 对象堆性能。
- 兼容层中的 `glBufferSubData`、映射和同步函数使用缓存的函数指针转发给原生驱动；普通 `glXSwapBuffers` 路径未无条件调用 `glFinish`。`glReadPixels` 发布路径仅在手动 Composite 重定向时启用，不能说所有游戏帧都会回读显存。
- 前一轮焦点同步修复只处理 `EVENT_FOCUS`，不会在每次挖掘或每次鼠标移动时写共享焦点记录。当前没有证据把挖掘掉帧归因于该修复。

## 建议的验证顺序

第一步只把游戏里的“区块构建器”从“半阻塞”改为“线程化”（对应 `prioritizeChunkUpdates:0`），在同一地点重复挖掘，对比帧时间尖峰，保持视距和内存等其他设置不变。线程化可能使方块破坏后短暂出现尚未更新完的画面，游戏自带说明也提示这一取舍。当前未代用户修改此项。

如果切换后尖峰明显减少，则优先调查同步区块编译及其分配、锁等待开销。如果仍然明显，则下一轮需要在用户自行游玩时采集渲染线程栈、帧时间及 GC / safepoint 时间：区分区块编译、地形缓冲上传/驱动等待和 JVM 暂停。再分别对渲染距离、模拟距离或堆上限做单变量对照；不同时更改所有选项后宣称找到了根因。
