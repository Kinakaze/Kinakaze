# Chrome / Firefox 启动推进：2026-09-22

本轮针对真实 Linux Firefox 140.16.0esr 和 Google Chrome 153.0.8010.52。
客体目录为 `artifacts/gnome-startup-root`；每次测试使用新 profile、本地 HTML 和独立 Windows Job，结束时回收该测试的子进程。
版本输出、图形能力探测、ABI 回归和页面渲染分别验收。

## 非无头模式实测与 XCB 修复：r15–r16

已使用独立新 profile 实际启动桌面模式，所有命令均未传入 `--headless`。
Firefox 创建了 900×650 的宿主可见窗口，但显示 `chrome://browser/content/browser.xhtml`
第 1 行第 1 列的 `XML Parsing Error: not well-formed`，还没有进入正常浏览器界面。
窗口截图为 `artifacts/browser-visible-r16/firefox-window-0.png`；r14 和 r16 均复现，
该结果不能计为桌面模式通过。

Chrome r14 在创建窗口前退出。原生调试捕获 `x11::Connection::Connection` 内的断点，
对应 `InitRootDepthAndVisual` 找不到根 visual。`xcb_get_setup` 原先只返回独立的固定头，
后续 vendor、format、screen、depth、visual 没有按 X11 wire 格式连续存放；Chrome
自行解码这块数据，因此使用硬编码全局数组的迭代器不能弥补该缺口。
r15 将其改为完整连续的 160 字节回复，并修正长度和迭代器步长。
r16 进一步修正空 visual 列表的末尾指针，以及调用者自有 setup 的 format 数量。

新增 `tests/guest/BrowserXcbSetupProbe.py` 直接解码原始回复，检查根 visual、格式、
各层迭代器地址/长度及调用者复制的数据；已纳入浏览器 ABI 验收。
旧 r14 在 `browser-r14-xcb-setup-baseline/abi.log` 失败，r15 与 r16 的 ABI 均通过，
最终修复包为 `artifacts/browser-fix-r16-dist`。r15 的 Chrome/Firefox 无头扩展页面
复测也都退出 0，证据为 `browser-fix-r15-boundary/results.json`。

修复后 Chrome 越过了上述初始化断点，并提供 DevTools 目标列表，但桌面模式仍未通过：

- 默认 GPU 路径随后报告 GPU 子进程 fork/启动失败，最终退出 127；见
  `browser-visible-r15/chrome.log`，未发现可见宿主窗口。
- 使用 `--disable-gpu` 的非无头诊断也未创建可见宿主窗口；CDP
  `Browser.getWindowForTarget` 返回宽高均为 0，见 `browser-visible-r15-software/cdp.log`。
  r16 再次启动仍无 Chrome 宿主窗口，记录于 `browser-visible-r16/observed-state.json`。
- 独立客体调用确认 `xcb_send_request` 对核心 `GetInputFocus`（43）及
  `CreateWindow`（1）均返回 `BadRequest`（1），见 `browser-xcb-raw-diagnostic.log`。
  当前仅有 XKEYBOARD 的原始请求分派；核心请求需要接入已有的原生窗口/属性等实现。

DevTools 目标存在不等于页面渲染和窗口交互正常。此前无头截图通过的结论仍限于无头路径；
桌面窗口、真实输入和 GPU 子进程需要继续验收。

## 已通过的无头基线：r14

最终验证包为 `artifacts/browser-fix-r14-dist`，包含 `init.exe`、`worker.exe` 和 29 个原生模块。
源码修改保留在工作区；独立构建快照避免其他任务同时编译造成二进制混用。

| 验证 | 结果 | 证据 |
| --- | --- | --- |
| 浏览器 ABI 与边界回归 | 通过，退出 0；含 fork 后新线程 TLS、非对齐 hint 的可执行映射及 fork 继承 | `browser-fix-r14-abi/results.json` |
| Chrome 多进程扩展页面 | 通过，25.4 秒，JS DOM 标记正确，退出 0 | `browser-fix-r14-boundary/results.json` |
| Firefox 扩展页面 | 通过，20.451 秒，800×600 PNG、JS 背景像素正确，退出 0 | 同上及 `browser-fix-r14-boundary/firefox.png` |
| Chrome CDP 截图与关闭 | 扩展页面截图成功，`Browser.close` 后退出 0 | `browser-cdp-render-r14.png/.log` |
| 相关单元测试 | 63 通过、0 失败、1 原有忽略 | `browser-r13-{tls,engine,pthread,eventfd}-unit.log` |

```powershell
python tools/test-browser-runtime.py --root artifacts/gnome-startup-root --dist artifacts/browser-fix-r14-dist --output-dir artifacts/browser-verify --only abi --only chrome-page --only firefox-page --session-bus --chrome-no-sandbox --page-file tests/guest/BrowserPageProbe.html --timeout 100
```

本轮页面验收使用本地测试页和无头模式；Chrome 因客体 root 身份明确使用 `--no-sandbox`。
这些结果不覆盖浏览器完整沙箱、DRM、视频会议或所有桌面门户服务。日志仍包含 Widevine
指令准备、udev、PipeWire/portal 等未阻断本次页面功能的诊断，不能视为这些组件均已正常。

## r12–r14 页面与边界验证

Firefox 已完成真实页面验收：`browser-fix-r12-firefox/results.json` 记录退出码 0，
18.966 秒生成 800×600 PNG；图片显示 `KINAKAZE_BROWSER_JS_42`，背景像素匹配 JS
设置的颜色。`browser-fix-r12-abi/results.json` 同时通过当时的全部 ABI 用例。

r10–r12 补齐 ELF 可执行代码的发现：使用异常展开索引定位去符号函数，识别有界相对跳转表，
并处理表基址在前一基本块、ADD 与间接 JMP 之间插入寄存器恢复的编译器形式。
这些分支包含 Firefox wasm2c XML 沙箱的 GS 指令；遗漏它们会触发间接调用检查并退出。
去符号夹具覆盖这些形式，旧包分别在 `browser-r10-hoisted-switch-baseline/abi.log`、
`browser-r11-switch-restores-baseline/abi.log` 失败，r12 通过。AOT 格式标记推进到 CRYAOT25。

Chrome 普通多进程在 r11 中已通过 CDP 返回本地页面的完整 DOM 和正确 JS 结果
（`browser-cdp-r11.log`），但 `--dump-dom` 仍超时，不能计为浏览器页面通过。
r12 原生调试捕获 renderer 在 V8 后台编译的 `LocalFactory::AllocateRaw` 中触发断点；
它读到错误的静态 TLS 初始值。fork 已传递模块模板和当前线程数据，却遗漏模块的静态偏移布局，
导致 fork 后创建的新 pthread 未正确初始化。r13 将布局、冻结状态与模板一起传递，并校验
模块编号、偏移、对齐和区间重叠。新增 `BrowserForkTlsProbe.c` 检查普通线程、子进程和孙进程
的新线程初值、零初始化及线程隔离；旧 r12 包稳定退出 9，证据为
`browser-r12-fork-tls-baseline/abi.log`。r13 的 ABI 回归全部通过。

`browser-fix-r13-pages/results.json` 中 Chrome 与 Firefox 页面均通过，分别在 29.667 秒、
26.176 秒以退出码 0 结束。Chrome 使用普通多进程模式，未使用 `--single-process`；
CDP 另取得 PNG 并成功执行 `Browser.close`，证据为 `browser-cdp-render-r13.png/.log`。
当前客体以 root 运行，Chrome 测试明确传入 `--no-sandbox`；浏览器默认配置未修改。

新增 `BrowserPageProbe.html` 覆盖 UTF-8、非对齐 DataView、数组排序/JSON、WebAssembly、
Canvas 像素与 PNG、本地存储和 DOM 事件。Firefox 在 r12、r13 均通过；Chrome r13
暴露 Wasm 代码页提交失败。`browser-r13-wasm-trace/chrome-page.log` 记录一个 8 KiB
非 64 KiB 对齐 hint 导致宿主保留范围与登记范围不符，后续 mprotect 返回 EIO。
`browser-r13-wasm-hint-baseline/abi.log` 在独立用例中重现。r14 对 hint 先按宿主粒度
保留，再拆分、释放前导间隙，确保登记范围与真实 placeholder 完全一致；回归覆盖
RW→RX 执行、fork 继承及页对齐 MAP_FIXED 复用。相关宿主约束见
[VirtualAlloc2 文档](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualalloc2)。

独立单元检查：TLS 20、执行引擎 35、pthread 栈 2、eventfd 6 项通过，0 失败；引擎原有
1 项忽略。证据为 `browser-r13-{tls,engine,pthread,eventfd}-unit.log`。TLS 检查包含
fork 布局恢复后新建线程的初值，以及拒绝重叠布局且保留原状态。

## 后续运行时修复（r4–r9）

以下修复继续使用真实浏览器定位，并由 `BrowserStartupAbiProbe` 检查实际 Linux 调用行为：

- 栈保护值统一读取客体 TCB，避免 FS canary 与镜像值不一致；FS/GS 指令跳板保留 SysV 128 字节 red zone。
- `_dl_find_object` 返回已装载 ELF 的映射范围、link map 和 EH frame。包含有/无 `PT_GNU_EH_FRAME` 的共享库及真实 GCC unwind 回归。
- fork 保留其他仍存活 pthread 的栈映射；退出时撤销映射及该栈上的 mutex 记录，修复子进程重建 mutex 时访问缺失栈。
- 维护每线程客体 GS base，翻译 GS 内存指令和 RDGSBASE/WRGSBASE；在线程创建和 fork 中继承状态，避免 Firefox wasm 访问 Windows TEB。
- getrlimit/setrlimit/prlimit 的客体缓冲区使用可捕获失败的复制，非法、只读、无权限及跨页地址返回 EFAULT；修复 Chrome 主动探测只读缓冲区时的原生异常。
- r9 修复 eventfd 的重复读写唤醒。Linux 每次写入唤醒 EPOLLIN，每次读取唤醒 EPOLLOUT；共享计数器同时保存通知序号，epoll 保存已消费序号并在 fork 中保留。参见 [Linux eventfd 实现](https://raw.githubusercontent.com/torvalds/linux/master/fs/eventfd.c)。
- r9 支持匿名 `MAP_FIXED|PROT_NONE` 跨越多个私有视图与保留区，预先校验完整覆盖，再清零目标内存并保留相邻页面，修复 Chrome PartitionAlloc 重新保留 2 MiB superpage 失败。

`browser-fix-r8-abi/results.json` 通过上述 r8 及之前的 ABI 项。r8 浏览器页面仍超时：Firefox 主线程停在 `audioipc2::ipccore::EventLoopHandle::add_connection` 等待通知；Chrome renderer 在 superpage 重新映射失败后触发断点。
新增边界用例在旧 r8 包上分别稳定失败，证据为 `browser-r8-boundary-baseline/abi.log` 和 `browser-r8-eventfd-baseline.log`。r9 的回归同时检查映射清零/相邻页面、eventfd 的 dup、线程、跨进程写入及多个独立监听器。

`browser-fix-r9-abi/results.json` 全部通过。Firefox 已越过 audioipc 等待，进一步暴露出 libxul `+0x44fd90c` 的未翻译 GS 指令；其函数只有一个 INT3 填充，但 `PT_GNU_EH_FRAME` 明确包含入口 `+0x44fd8f0`。r10 将异常展开索引作为扫描根，并识别有边界检查的相对跳转表，覆盖间接调用及 switch 分支。相应去符号夹具在旧 r9 上返回错误数据，证据为 `browser-r9-unwind-baseline/abi.log`。

`browser-fix-r9-chrome-single/results.json` 的单进程诊断已输出正确 JS DOM（`marker_found: true`），但退出码为 191，仍判失败；普通多进程模式仍超时。单进程参数仅用于定位，不作为默认运行方式或可用验收。r10 起使用 `artifacts/browser-source` 独立源码快照构建，避免其他任务同时重编译共享 target 导致包内二进制不一致。

下面的 r3 结果保留为早期基线，不代表后续运行时修复的最终验收结果。

## 已修复的入口与行为

| 范围 | 修复 |
| --- | --- |
| X11 | `_XGetScanlinePad`、`_XGetBitsPerPixel`、`XListExtensions`、`XFreeExtensionList`；Display 的内部 ScreenFormat 布局与公开 PixmapFormat 区分，发布真实支持的格式和扩展 |
| XCB | `xcb_send_fd`，并修正批量 FD 请求：接管并关闭 FD，当前不支持的传输返回连接错误 7；错误状态参与 fork 快照，flush/request 不再假报成功 |
| libm | 接入当前 binary80 `powl` 实现，以真实 Linux C 调用者检查 x87 返回 ABI、超出 double 的精度/指数范围、特殊值及控制字保持 |
| RandR | `XRRGetProviderResources`、`XRRGetProviderInfo` 及对应两个 free；查询现有 Windows 主输出，返回正确 LP64 布局和可释放快照，不宣称 PRIME/offload 或完整 RandR 1.4 |
| SwiftShader 的旧 pthread ABI | 为 `libpthread.so.0` 提供 `pthread_mutexattr_setpshared@GLIBC_2.2.5` 转发；非法参数返回 EINVAL，目前不支持的进程共享模式返回 ENOTSUP，保留 mutex type |
| memfd | 拒绝未知 flags 和超长 name；修复 Chromium 用 `memfd_create("", ~0)` 探测内核能力时错误返回 FD 导致的 PCHECK 崩溃 |
| procfs | 查询 `/proc/<pid>/task/<tid>` 时校验数字 TID、所属进程和线程存活；本进程 task 目录的链接数反映活跃客体线程数量 |
| root 准备 | 创建 `/dev/shm` 实际后端目录，解决 Chrome 创建共享内存时报 ENOENT |

XCB 的 FD 所有权和错误 7 依据 [XCB 接口定义](https://xcb.freedesktop.org/manual/xcbext_8h_source.html)。
memfd 的非法标志/名称错误依据 [Linux memfd_create 手册](https://man7.org/linux/man-pages/man2/memfd_create.2.html)。
Chrome 对 task 存活和链接数的使用可见 [ThreadHelpers](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/sandbox/linux/services/thread_helpers.cc)。
这些修复不代表完整 memfd sealing、跨进程 pthread mutex、procfs 线程目录枚举或 GPU FD 传输已经完成。

## 可复现验证

```powershell
./tools/build.ps1 -Release -RefreshExports -SkipFormat -SkipTests -DistDirectory artifacts/browser-fix-r3-dist
python tools/test-browser-runtime.py --root artifacts/gnome-startup-root --dist artifacts/browser-fix-r3-dist --output-dir artifacts/browser-fix-r3-core --only abi --only firefox-version --only chrome-version --only firefox-glxtest
python tools/test-browser-runtime.py --root artifacts/gnome-startup-root --dist artifacts/browser-fix-r3-dist --output-dir artifacts/browser-fix-r3-pages --only firefox-page --only chrome-page --session-bus --chrome-no-sandbox --timeout 50
python tools/native-exports/audit.py --observe artifacts/gnome-startup-root/opt/firefox --observe artifacts/gnome-startup-root/opt/google/chrome --dist artifacts/browser-fix-r3-dist --output artifacts/browser-fix-r3-imports.json
```

页面探针使用带 JS 计算的本地 HTML。Chrome 必须输出计算后的 DOM；Firefox 必须退出成功、生成截图并命中背景像素。失败/超时会产生非零退出码，不计作通过。
`--chrome-no-sandbox` 仅用于当前 root 客体的诊断，不修改浏览器默认配置；没有这个参数时，Chrome 按自身策略拒绝 root 启动。
`--chrome-arg=--no-zygote` 是定位派生进程问题的额外诊断，报告会记录完整参数。

## 修复前的可重复失败

- `browser-fix-v81-baseline`：Firefox 缺少 `_XGetScanlinePad`，Chrome 缺少 `xcb_send_fd`。
- `browser-fix-r2-regression-before`：raw `SYS_memfd_create` 非法 flags 回归失败。
- `browser-fix-r3-regression-before`：task 目录链接数仍为 1，回归失败。
- `browser-fix-r1-firefox-long`：没有独立 D-Bus 会话时 120 秒无截图；trace 显示进入 dbus-launch 自动启动路径。
- `browser-fix-r1-chrome-shm`：创建 `/dev/shm` 后越过 ENOENT，定位到 memfd 能力探测断言；日志中的 Broken pipe 是当时残留 errno，不能据此认定 Mojo 管道实现是根因。
- `browser-fix-r2-pages`：Firefox 越过 glxtest 导入失败后触发 abort；Chrome 越过 memfd 断言后 GPU 子进程启动失败。
- `browser-fix-r2-chrome-nozygote`：明确复现 `Stopped thread does not disappear in /proc`，对应本轮 procfs 修复。

完整版本化导入审计覆盖两款浏览器目录的 39 个 ELF、2,429 项需求，包含 SwiftShader 等按需装载组件。另对 13 个主程序/辅助程序入口做 DT_NEEDED 和强符号闭包检查。上述静态检查不覆盖所有未来 dlopen 分支，也不等同于页面成功。
客体回归源码位于 `tests/guest/BrowserStartupAbiProbe.c` 和 `.py`，运行器为 `tools/test-browser-runtime.py`。每份运行结果保存 worker 和所有原生库的 SHA-256。

## r3 阶段构建与结果

最终包：`artifacts/browser-fix-r3-dist`。完整构建脚本返回 0，导出生成检查返回 0（29 个模块、5,605 个导出）。当前工作区还有并行进行的音频/分配器开发，结果以测试 JSON 中的二进制哈希为准。

| 验证 | 结果与证据 |
| --- | --- |
| 浏览器 ABI | `browser-fix-r3-abi/results.json`：通过；包含 long double、memfd libc/raw syscall、线程存活/退出/link count、X11、RandR、XCB、pthread 兼容入口 |
| Firefox --version | `browser-fix-r3-core/results.json`：通过，140.16.0esr |
| Chrome --version | 同上：通过，153.0.8010.52 |
| Firefox glxtest | 同上：退出 0，实际取得 NVIDIA RTX 4060 Laptop GPU / OpenGL 4.6 的 GLX 报告；同时仍报告部分 EGL 方法缺失 |
| 版本化导入 | `browser-fix-r3-imports.json`：39 个 ELF、2,429 项需求，缺失为 0 |
| 主程序/辅助程序强导入闭包 | `browser-fix-r3-closure.json`：13 个入口，缺少文件/强符号均为 0；不包含所有 Qt/portal 等桌面服务分支 |
| 导出工具测试 | `browser-fix-r3-export-tests.log`：21/21 通过 |
| 客体依赖工具测试 | `browser-fix-root-tests.log`：20/20 通过；另直接运行 RootPlan.install 验证新 root 创建 dev/shm |
| Firefox 本地页面 | `browser-fix-r3-pages/results.json`：失败，退出 11，无截图；仍有 mozalloc_abort，旁路启动的 portal 缺少 pw_core_get_client |
| Chrome 本地页面 | 同上：50 秒超时，无目标 DOM；已越过先前的 memfd/线程检查，日志在 Chrome ELF 偏移 `0x395cb53` 报栈保护失败，根因尚未确认 |
| Chrome no-zygote 对照 | `browser-fix-r3-chrome-nozygote/results.json`：30 秒超时，未再出现已退出线程仍在 procfs 的断言；后续出现 `/proc/self/exe` 的 execvp 失败，仍未输出目标 DOM |

`browser-fix-r3-core` 中最初的 ABI 项在 Python 线程完成锁已释放、native pthread 尚未结束析构的时间窗口内过早检查。已将探针改成与 Chromium 相同的有界等待语义（最多 2 秒），修正后的独立 ABI 结果为 `browser-fix-r3-abi`，没有放宽失败线程最终必须消失的要求。

浏览器仍未达到页面可用验收。后续应从 Firefox abort、Chrome 栈保护失败和 portal 的 PipeWire 入口继续定位。
