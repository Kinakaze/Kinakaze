# DSH、zcode、OpenCode 与现有 agent 验证

测试日期：2026-09-30。完整报告和原始日志在 `artifacts/additional-agents-20260930/`。测试使用 Linux x86-64 程序，运行于 Kinakaze 客体；模型协议服务仅绑定本机，使用确定性工具调用夹具和假凭据。没有使用线上模型账户。

## 本轮结果

| 工具 | 版本 / 来源 | 启动与工具结果 | 报告目录 |
| --- | --- | --- | --- |
| OpenCode | 1.18.33，npm `opencode-linux-x64` | `--version`、`--help` 通过；任务运行时未发出模型请求。早期运行超时，后续运行因系统剩余物理内存低于 8 GiB 被保护性中止；12 项工具动作未完成。 | `opencode-startup-r3`、`opencode-memory-aware-workflow` |
| zcode | [zerx-lab/zcode](https://github.com/zerx-lab/zcode)，17.2.15-z1 | `--version`、`--help` 通过；任务运行在 100 秒时超时，未发出模型请求，12 项工具动作未完成。 | `zcode-startup-r3`、`zcode-memory-aware-workflow` |
| dsh | [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)，0.2.0-rc.2 | 版本、顶层帮助、headless 配置导出通过；headless 启动失败。Node 22.23.3、24.21.0 均报 `Unsupported/no-realm`。 | `dsh-startup`、`dsh-node24-startup` |
| Codex | 0.157.1 | 18 项真实工具动作及宿主独立验证通过。 | `codex-control-r3` |
| pi | 0.73.1 | 16 项真实工具动作及宿主独立验证通过。 | `pi-control` |
| Claude Code | 2.1.283 | 读取源文件通过，第二项列目录时 `posix_spawn /bin/bash` 返回 ENOMEM；19 项动作未全部完成。 | `claude-control-r3` |

启动通过不表示工具闭环通过。OpenCode、zcode 没有进入本地模型请求阶段，因此其模型协议应答和后续文件修改断言尚未得到实际 CLI 验证。Claude 的具体失败输出保留在报告中，不能将其视为完整通过。

## 测试环境与证据

运行库来自本轮开始时的 `artifacts/agent-compatibility-20260930/checked-remove-candidate`，复制为本轮独立的 `runtime/`；报告记录实际运行库与 CLI 文件 SHA-256。后续工作区中的修改不会自动进入这个运行库快照。

OpenCode 下载了 npm 发布的 Linux 包，核对 registry 提供的归档 SHA-1；zcode 核对发布页的 `SHA256SUMS.txt`。Node 24.21.0 的 Linux 归档核对官方 `SHASUMS256.txt`。下载地址、文件大小和 SHA-256 保留在 `download-report.json` 与 `node24-download.json`。

dsh 的 npm 依赖树在本轮专用客体目录中按 Linux x64 glibc 目标准备；安装禁用生命周期脚本，实际使用发布包携带的原生扩展。报告中的 `Unsupported/no-realm` 来自成功加载后的扩展运行时探测，不是缺少 `.node` 文件。该安装方式不代表首次在线安装和所有生命周期脚本均已验证。

原始初始化诊断保留在 `opencode-trace/`、`opencode-owned-stacks/` 和 `zcode-memory-aware-workflow/session/`。这些记录只证明观测到的初始化阻塞或资源压力；没有断言某个等待栈就是最终根因，也没有修改兼容层来掩盖失败。

## 内存与进程清理

`tools/agent_resource_watch.py` 只采样所属 Windows Job 中的进程，按进程句柄固定身份；句柄和采样线程在结束时释放。它不会复制 Job 句柄，避免延长 Job 的生命周期。采样记录有界，报告记录受管进程退出后的状态。

最终资源检查覆盖 OpenCode、zcode、dsh（Node 24）、Codex、pi、Claude：采样器未报错，`alive_after_close` 均为空。超时和内存保护结束测试时关闭所属 Job；没有按全局进程名称清理其他会话。

早期按合计工作集设置的 4 GiB 上限过于保守，共享驻留页可能被不同进程重复统计；这批中止报告仍保留，不能据此判定工具不兼容。随后改用私有提交量与系统剩余物理内存作为中止条件。较小的 1 GiB 私有提交上限也中止了 Codex / Claude 的初始补充运行；最终补充运行使用 8 GiB 上限。每份资源报告记录该次实际阈值。

OpenCode 的后续运行触发了系统剩余内存低于 8 GiB 的保护，不能宣称其任务运行内存开销正常。zcode 的最终运行超时，且没有触发内存阈值。退出清理通过不证明进程长期运行没有内存泄漏；本轮未完成长期稳定性和原生 Linux 对照。

## 复现入口

输出目录必须是新目录，以保留上一轮的失败和日志。

```powershell
python tools/test-extra-agent-startup.py --agent opencode --root <客体根> --dist <运行库快照> --output <新报告目录> --cli /opt/additional-agents-20260930/opencode/package/bin/opencode
python tools/test-extra-agent-startup.py --agent zcode --root <客体根> --dist <运行库快照> --output <新报告目录> --cli /opt/additional-agents-20260930/zcode
python tools/test-extra-agent-startup.py --agent dsh --root <客体根> --dist <运行库快照> --output <新报告目录> --cli /opt/additional-agents-20260930/dsh/node_modules/@deepseek-ai/dsh/lib/bin.js --node /opt/additional-agents-20260930/node-v24.21.0-linux-x64/bin/node
python tools/test-openai-agent-workflow.py --agent opencode --root <客体根> --dist <运行库快照> --output <新报告目录> --cli /opt/additional-agents-20260930/opencode/package/bin/opencode --timeout 100
python tools/test-openai-agent-workflow.py --agent zcode --root <客体根> --dist <运行库快照> --output <新报告目录> --cli /opt/additional-agents-20260930/zcode --timeout 100
```

zcode 的工具夹具选择其受支持的 `replace` 编辑模式，并关闭标题生成、扩展、技能、规则及 LSP 发现；没有验证默认 hashline 编辑和这些扩展功能。Codex、pi、Claude 使用现有 `test-agent-tool-compatibility.py`；本轮资源采样包装器与完整启动参数保留在报告目录。

新增脚本通过 Python 语法检查和 Ruff 检查，工作区 `git diff --check` 通过。这不是完整 Rust 构建或全仓库测试结果。
