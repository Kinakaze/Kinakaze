# Agent 运行时修复与验证（2026-09-30）

本轮继续修复 `additional-agent-validation-2026-09-30.md` 中的失败。测试使用本地确定性模型服务和假凭据，执行真实 CLI 的文件、搜索、编辑和 shell 工具；不代表在线模型质量或真实账号登录验证。

## 修复

- `msync` 通过实际虚拟地址空间验证覆盖范围，接受已提交的原生堆、代码和栈页。此前只查 `mmap` 登记表，导致 dsh 的 `node-addon-require-builtin` 可读性探测返回 `ENOMEM`、报 `Unsupported/no-realm`。
- 整段私有匿名 section 的 `MADV_DONTNEED` 改为替换零填充 backing，保留地址、权限及 fork 元数据，释放旧 backing。512 MiB 稀疏映射的丢弃不再写满全部页；部分区间和 COW 映射沿用原来的处理路径。
- 至少 4 GiB 的非固定私有匿名映射改用受 Windows Job 提交限额约束的私有分配。内存不足和提交配额错误统一返回 `ENOMEM`，使 Bun 的投机大分配能进入正常降级路径。
- 异步信号是否可递送以当前屏蔽集为准：处理函数内的 `sigsuspend` 可以接收同一信号；`SA_NODEFER` 仍应用显式 `sa_mask`。
- `pthread_cond_wait` 和定时等待检查待处理信号，在互斥锁释放期间调用处理函数，返回前重新获取锁。此前 Bun/JSC 等待目标线程确认暂停，目标却一直停在不递送信号的条件变量等待中。
- 无 `FUTEX_PRIVATE_FLAG` 的 futex 接受加载器登记的 ELF COW 映像内存，并使用进程私有键。此前 Windows 将这些页标为 `MEM_MAPPED`，地址归属检查误报 `EFAULT`，导致 dsh 自带的静态 ripgrep 在并行目录搜索时反复重试。普通共享映射继续使用原有共享键。
- 执行引擎追踪条件选择的回调指针，以及跨局部分支的寄存器间接调用。修复 musl 线程退出和 Rust 调用 `readdir` 时漏接管系统调用的问题；AOT 缓存版本升至 36，已有错误扫描结果会失效重建。新增测试同时验证覆盖寄存器后不再接受旧目标、执行段内的数据不会被误作代码。
- dsh 的安装布局修复另见下节。测试适配器补齐 zcode 必填意图字段、不同编辑字段名和错误文本，并固定输入文件为 LF，避免 Windows 自动换行造成错误判定。

## 已完成的回归

最终运行库：`artifacts/agent-fixes-20260930/candidate-r7`。CLI 的 JSON 报告均保留运行库和 CLI 哈希；早期失败目录保留。六个 CLI 合计 94 项工具操作通过。

| 工具/测试 | 结果 | 证据目录或日志（相对本轮 artifacts） |
|---|---|---|
| dsh 0.2.0-rc.2 | 12/12，两次完整通过 | `dsh-final-r7b`、`dsh-final-repeat` |
| OpenCode 1.18.33 | 12/12；此前 r4 两轮、r5 一轮也通过 | `opencode-final-r7` |
| zcode 17.2.15-z1 | 12/12，包含全新 HOME 的原生扩展解包 | `zcode-final-r7` |
| Codex | 21/21，含 MCP | `control-final-r7/codex-0` |
| Claude Code | 21/21，含 MCP | `control-final-r7/claude-0` |
| pi | 16/16 | `control-final-r7/pi-0` |
| 内存专项 | 通过 | `memory-final-r7` |
| 同一信号嵌套与 SA_NODEFER | 旧版本超时，修复后通过 | `nested-signal-before`、`nested-signal-final-r7` |
| 条件变量信号与锁恢复 | 旧版本失败，修复后通过 | `condition-signal-before`、`condition-signal-final-r7` |
| ELF 私有 futex 与共享 futex | ELF 最小复现在 r4 失败；最终全部通过，含 150 次跨进程握手 | `elf-futex-before`、`futex-final-r7` |
| 执行引擎单元测试 | 48 通过、1 忽略 | `guest-tests-final-verified.log` |
| VFS 信号单元测试 | 18 通过 | `signal-tests-r4.log` |
| 当前工作区 Cargo 检查 | 通过 | `integration-check-final.log` |

编译检查使用当前工作区全部库和 `kinakaze-v2-runtime/guest-engine` feature。可执行回归使用隔离的 source/target，包含本轮改动和此前 agent 兼容修复，避免混入其他会话正在编辑的 ptrace/remap 实现；没有把隔离回归称为当前所有未提交修改的完整集成测试。

## 内存与资源

- 512 MiB 稀疏映射丢弃后，触碰之前 `mincore` 报告 0 字节驻留。测试还覆盖部分页、只读/不可访问权限及 fork 后父子数据隔离。
- 4 GiB Job 内申请 8 GiB 匿名映射返回 `ENOMEM`。重复丢弃 76 次；一次完整交互预热后，三个检查点为 293–294 句柄、5 线程、157138944 字节私有提交，没有持续增长。
- OpenCode 在 16 GiB Job 内启动 shell 快照会返回 `ENOMEM`；24 GiB 上限下工作流通过，最终采样私有提交峰值为 17049219072 字节。zcode 为 9531629568 字节，dsh 为 583389184 字节。测试默认分别采用 24/16/16 GiB，可显式覆盖。
- dsh、OpenCode、zcode 及内存专项的最终资源报告均显示 `cleanup_passed: true`、无资源保护触发、`alive_after_close: []`。Codex、Claude、pi 的矩阵报告记录退出码和 Job 内存指标。测试拥有的进程通过 Job 清理；不按全局进程名称终止其他会话。
- 保护器同时检查剩余物理内存和系统剩余提交额度，阈值各为 8 GiB。采样器限制进程句柄数量，不保留无限时间序列。
- Windows 的原始 Job 峰值计数在拒绝 128 GiB 投机分配时也可记录该请求；不能把这一计数当作实际驻留或采样私有提交。较小的匿名 section 仍具有 Windows 特有的提交计账，测试不能证明所有分配路径都受同一种限额约束。

这些是有界重复测试及退出清理证据，不构成长时间运行绝无泄漏的证明。OpenCode 的 fork 快照内存开销仍较高。

## dsh 安装布局

dsh 0.2.0-rc.2 安装中存在 8 份源码完全相同的 `@deepseek-ai/dsh-tools`。工具调度器使用模块局部 `Symbol`；插件和 agent loop 加载不同副本时出现 `Cannot read properties of undefined (reading 'prepare')`。

`tools/repair-dsh-tool-layout.py` 校验整个包文件清单一致，将 7 份重复目录移入专用备份目录，并建立指向 `dsh-base` 下唯一副本的客体符号链接。没有修改发布包源码。操作计划、全部文件哈希、备份位置及执行结果保存在 `dsh-layout-repair`；常规 `npm dedupe` 在依赖解析阶段被有界停止，不能视为成功去重。npm 的去重机制说明见[官方文档](https://docs.npmjs.com/cli/v11/commands/npm-dedupe/)。

## 重跑

```powershell
python tools/test-openai-agent-workflow.py --agent opencode --root <guest-root> --dist <dist> --output <new-output> --cli /opt/additional-agents-20260930/opencode/package/bin/opencode
python tools/test-openai-agent-workflow.py --agent zcode --root <guest-root> --dist <dist> --output <new-output> --cli /opt/additional-agents-20260930/zcode
python tools/test-openai-agent-workflow.py --agent dsh --root <guest-root> --dist <dist> --output <new-output> --cli /opt/additional-agents-20260930/dsh/node_modules/@deepseek-ai/dsh/lib/bin.js
python tools/test-agent-memory-advice.py --root <guest-root> --dist <dist> --output <new-output>
```

完整工具回归不启用 `--trace`：原生系统调用诊断会插入子进程 stderr，打断程序输出中的连续校验标记。诊断运行保留作定位证据，最终通过结论来自未开启诊断的运行。
