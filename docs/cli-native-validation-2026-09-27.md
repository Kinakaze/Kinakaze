# libc 正则与交互式 CLI 验收（2026-09-27）

本轮修复已打入 `artifacts/cli-fixed-release-assets/Kinakaze-0.1.0-preview-windows-x86_64.zip`。
这是本地候选包，未上传发布。大小为 21,873,301 字节，SHA-256：

```text
d6e6c1f78540e1cf77f126436006058ee5c5d12afeca66785b5e5b9803b917e1
```

## 正则修复

`libs/libc/src/regex.rs` 修正 glibc 的 `REG_NEWLINE=4`、`REG_NOSUB=8`，
补齐连接、分支、重复和捕获组之间的回溯及最左最长选择，支持反向引用，
修复空匹配重复终止、大小写折叠、否定字符类、换行锚点、字符类错误码、
GNU 反向搜索与 `re_match` 返回长度。长输入的重复匹配使用迭代处理。

10 项回归测试全部通过。另在实际 Debian Bash 中运行 Claude 安装器使用的
清单正则，成功提取完整 64 位 SHA-256，退出码为 0。
该实现仍以字节为单位；本次未宣称覆盖所有 locale、多字节字符和 GNU 语法扩展。

## 交互式运行

测试使用真实 Windows ConPTY，执行 Linux ELF 二进制，并实际发送方向键、回车和 Ctrl+C。
开发侧 Python 只控制终端与记录结果，不属于发行包依赖。

| 项目 | 结果 |
| --- | --- |
| Codex 0.157.1（Linux musl） | 默认启动完成后台服务安装、进入登录菜单；方向键改变选项，Ctrl+C 退出码 0 |
| Codex `--no-daemon` | 登录菜单、方向键、窗口调整和 Ctrl+C 退出通过 |
| Claude Code 2.1.283（Linux glibc） | 显式设置 guest HTTP/HTTPS 代理后通过联网检查；回车进入登录方式选择，方向键改变选项，Ctrl+C 最终退出码 0 |
| Claude Windows 2.1.283 对照 | 同样的 ConPTY 与代理；关闭界面后需要再次 Ctrl+C 才退出的现象也可复现，最终退出码 0 |
| Bun 1.4.2 网络探针 | HTTPS fetch 返回 200；HTTP CONNECT 后 TLS 1.3 握手与读取通过 |

没有导入主机的登录凭据，没有执行已认证的模型请求。CLI 代理测试明确在 guest
环境设置 `HTTPS_PROXY` 和 `HTTP_PROXY`；宿主环境变量不会自动全部传入 guest。
发行安装器的系统代理行为另由首次联网启动验收覆盖。

启动过程中同时修复了缺失的 quick-exit ABI、调用者提供的 pthread 栈与线程退出、
缺少 PT_PHDR 的静态 PIE 辅助向量、epoll 复制描述符及控制台就绪事件、
`epoll_pwait2` 超时结构、带子栈的 vfork，以及 AOT 漏掉的 GOT/回调/分支表入口。
加载器不再重置父进程共享控制台的输入模式，避免启动辅助程序后方向键与 Ctrl+C 失效。

回归结果包括：正则 10 项、pthread 41 项、epoll 28 项、AOT 分段修补 22 项
（另 1 项性能测试未运行）、入口识别 5 项、系统调用跳板 1 项、pwait2 1 项。
`tests/guest/RawVforkStackProbe.S` 实际验证父子栈、子进程退出和 exec，返回
`RAW_VFORK_STACK_OK`。原生导出检查通过：29 个链接输入、5,907 个导出。

## 发行包

最终包携带 `init.exe`、`worker.exe`、应用本地 DLL、native 提供者、
`rootfs.manifest.json` 和启动脚本。首次启动由原生安装器安装清单锁定的 302 个
Debian 包，不依赖用户安装 Python。

最终发行验收共 12 项全部通过，覆盖空 PATH、新 rootfs、默认登录 shell、
并发初始化、含空格路径、外部清单、代理与环境变量覆盖、保留用户配置、
父子进程与退出状态。此次回归复用已校验的 Debian 下载缓存；从空缓存联网
下载的独立记录见 `first-run-online-validation.md`。

证据目录：`artifacts/cli-interactive-20260927/`。主要文件：

- `regex-r11-tests.log`、`regex-installer-final.log`
- `codex-final.jsonl`、`claude-proxy-final.jsonl`、`host-claude-283-exit.jsonl`
- `raw-vfork-final.log`、`pwait2-tests.log`、`code-roots-r11-tests.log`
- `final-release-validation.json`、`final-exports-check.log`

候选运行库目录为 `artifacts/cli-native-r13/`，未初始化的发行目录为
`artifacts/cli-fixed-release/`。测试 rootfs 位于
`artifacts/full-first-launch-final-20260927/Kinakaze/rootfs/`，测试下载的 CLI 位于
该 guest 的 `/root/cli-smoke/`，不包含在发行 ZIP 中。
