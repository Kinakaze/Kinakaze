# Agent 与开发工具链验证

本轮基于 v0.7.0，修改尚未发布。主要目标是实际工具调用、编辑和测试闭环，以及全新 Debian 根中的开发工具可用性。测试使用独立样例和本地确定性模型协议服务，不代表线上模型质量、账号认证或所有第三方插件通过。

## 已定位并修复的问题

- Claude 2.1.283 的安全文件操作使用 `/proc/self/fd/<目录>/<文件>`。原先 mkdir 返回 EINVAL，open/stat/rename 也缺少相应路径处理，导致 Bash 无法读取输出、Edit 无法原子替换文件。现在通过目录描述符解析中间组件；验证已关闭描述符、O_NOFOLLOW、目录重命名、tmpfs 创建及原子替换。tmpfs 的旧路径已消失时继续使用保留的 inode。
- Codex 0.157.1 的 shell 准备命令出现约 38,500 UTF-16 字符的宿主命令行，触发 Windows CreateProcess 上限，表现为间歇性的 execve ENAMETOOLONG。Linux argv 现在作为独立段进入既有进程交接映射，execve 和 posix_spawn 都使用短宿主启动参数；不写入临时脚本或环境变量。验证长字符串、引号、换行、空参数和 Unicode。
- 新安装的 GCC、Clang、Cargo 之前会把原生 PE 库作为 ELF 链接输入。首次安装现在在 Debian 多架构目录提供生成的 ELF 符号接口，原始 PE 仍保留在原生库位置。动态加载器的链接接口放在 `/usr/lib/x86_64-linux-gnu`，保留 `/lib64` 的可执行解释器。接口来自同一构建的真实导出。
- CMake 缺少 `__wmemcpy_chk@GLIBC_2.4`。新增按 wchar 元素计数检查的实现，并验证边界哨兵、零长度和超界 SIGABRT；版本证据来自实际 CMake ELF。
- 当前进程的 `/proc/self/cmdline` 使用本进程初始 argv，不再受共享进程列表的 512 字节字段限制。其他进程的长 cmdline 仍受该字段限制；没有声称已经实现跨进程任意长度或应用原地改写 argv 后的全部 Linux 语义。

## 可复现入口

所有报告位于 `artifacts/agent-toolchains-20260929/`，保留失败、中间版本和最终版本。兼容性候选包为 `final-candidate/`，进一步减少目录查询的性能候选为 `directory-candidate/`；可首次安装的最终本地运行目录为 `ready-runtime/`，其验收为 `ready-runtime-validation.json`。这些不是新的正式 release。测试记录包含运行库 SHA-256。`tools/init_pool.py` 已同时记录便携 `native/` 布局与开发 `rootfs/lib` 布局的实际文件哈希。

```powershell
python tools/test-codex-workflow.py --root <测试根> --dist <候选包> --output <报告目录> --codex <客体绝对路径>
python tools/test-claude-workflow.py --root <测试根> --dist <候选包> --output <报告目录> --claude <客体绝对路径>
python tools/test-pi-workflow.py --root <含 pi 和 Node 的测试根> --dist <候选包> --output <报告目录>
python tools/test-server-features.py --root <测试根> --dist <候选包> --output <报告目录> --repeat 3
python tools/test-common-workloads.py --root <开发根> --dist <候选包> --output <报告目录> --only '^(gcc|clang|cpp-futures|go|cargo|cmake-ninja|python-venv|npm-lifecycle|ruby|php|node|python|apt-check)$'
```

Agent 协议测试要求真实读取源文件、先运行失败测试、修改实现、再运行成功测试，宿主独立检查最终源码和测试文件未被篡改。CLI 的成功文本或零退出码本身不能让测试通过。

开发工具测试包括 GCC/Clang 动态库与线程、C++ future、Cargo 离线单元测试及无改动重建、CMake/Ninja/CTest、Python venv/pip、npm pretest/test 子进程、Go、Ruby、PHP 和 Node 的线程、文件、网络等现有场景。旧 `common-root` 的过时基础包导致 APT 文件冲突，保留失败后改用新建根；不通过强制覆盖隐藏冲突。

`online-final-validation.json` 验证新增链接接口的首次启动、独立外部 manifest、空 PATH、并发复用和进程树清理。`default-toolchain-validation/results.json` 在该清单初始化后真实 APT 安装编译器，GCC、Clang、C++ future、Cargo、CMake/Ninja 和 apt-check 两轮通过，不依赖手工替换头文件或编译库。

最终兼容性报告：`final-codex-1..3` 和 `final-claude-1..3` 六轮闭环全部通过；`final-pi` 的真实 pi 0.73.1 工具闭环通过；`features-final` 十八项探针通过；`toolchains-final-image` 十三项场景通过，`final-java` 的 OpenJDK 17 编译、线程和子进程场景通过。`final-full-tests.log` 中 2016 项 Rust 测试通过，29 项既有忽略，原生导出、格式、工具测试和 worker smoke 通过。初始失败及中间版本报告均未删除。

最后的目录查询优化另有 `bridge-directory-tests.log` 的 18 项测试；`optimized-codex`、`optimized-claude`、`optimized-pi`、`optimized-features` 九项探针和 `optimized-toolchains` 十三项工具链全部通过。

`optimized-terminal/report.json` 验证 Bash 前台任务中断、原始 Ctrl+C 输入、Claude 交互界面双 Ctrl+C 退出以及返回后继续输入命令。该工具的离线编辑步骤是确定性夹具，不替代上面的真实 CLI 工具闭环。

## 性能口径

`daily-performance/results.json` 是 v0.7.0 与修复后的 argv 候选包在同一根上的配对比较：15 类场景，每版一次预热、五次正式样本，180 个样本全部通过。变化方向混合，不能据此宣称通用提速：20 次进程启动中位数约 1775→1791 ms，文件树 1107→1044 ms，Git 1833→1821 ms，链接操作 742→788 ms。

随后针对 provider 发现减少临时导出列表，只保留三个管理标记；仍校验所有未保留导出的名称、序号、地址与区段。字符串检查改为在其所在区段内一次有界扫描，避免再次定位同一字符串。针对丢弃条目的错误序号和空地址新增回归，bridge 共 18 项测试通过。启动测量和开启诊断的分阶段采样分开执行。

只做导出列表和字符串扫描调整时，四十次正式启动中位数为 72.220→72.312 ms，不能称为提速。后续复用目录枚举已有的文件类型，避免每个普通原生文件再调用 is_file；符号链接保留跟随检查。`directory-startup/results.json` 的四十次正式样本、三次预热采用交替版本顺序，中位数 **75.976→71.867 ms（约 5.4%）**。最大值 **104.436→107.962 ms**，并未改善最慢样本。独立十次诊断的 provider 发现中位数 **5.305→3.924 ms（约 26%）**，见 `directory-profile/`。这些是同机局部测量，不是原生 Linux 对照。

`directory-daily/results.json` 的 120 个样本全部通过，但三个正式样本时部分文件/Git 结果变慢。保留该报告并扩大到每版每场景十二次正式样本、两次预热：`files-performance-repeat/results.json` 中文件树中位数 **1265.439→1193.674 ms**，Git **2202.484→2102.531 ms**；全部 56 个样本通过。文件树 p95 仍从 **1333.657 增至 1431.861 ms**，不能宣称尾延迟全面改善，也不把不同轮次的绝对时间直接拼接比较。

本机没有同硬件原生 Linux 对照，不能证明已比肩 native 或完成“极致优化”。后续仍需扩大 agent、插件、真实依赖构建与交互压力测试范围；没有将这批测试外推为所有 agent、所有语言版本或所有 Debian 命令均兼容。

本轮没有验证 Gemini CLI、OpenCode、在线账号认证或第三方 MCP 服务；当前 Rust/Cargo、Go 等系统工具链使用 Debian bookworm 软件包，不能外推为所有最新上游版本已通过。
