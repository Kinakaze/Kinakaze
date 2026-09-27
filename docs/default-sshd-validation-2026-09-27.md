# 默认 Debian SSH 验证（2026-09-27）

默认在线清单锁定 306 个 Debian bookworm 包，新增 OpenSSH 服务端、SFTP 服务端、libwrap0 和 runit-helper。默认登录 shell 自动启动 sshd，仅监听 `127.0.0.1:2222`，账号 `root`，密码 `kinakaze`。首次启动创建各安装实例独有的 RSA、ECDSA、ED25519 主机密钥；私钥和 `/etc/shadow` 权限均为 0600。配置与启停方式见 [首次安装](first-run.md#默认-ssh-登录)。

在 Windows x86-64 上使用全新 rootfs 验证，安装器复用已校验的 Debian 下载缓存。测试端使用 Windows 自带 OpenSSH 客户端与真实 ConPTY 输入密码；Python 和 pywinpty 仅用于开发机测试，发行包不包含或调用宿主 Python。

通过 `tests/guest/run-default-sshd.py` 的 7 项验收：

- 默认登录自动启动，首次生成三类主机密钥；另与独立安装实例比较，确认密钥不同。
- 错误密码被拒绝，指定命令没有执行。
- 默认密码登录为 root；配置、文件权限、远程管道和退出码 23 正确。
- 分配 `/dev/pts/` 终端，交互 shell 正常，退出码 17 正确。
- SFTP 上传、下载与删除成功，9,472 字节二进制内容逐字节一致。
- 退出默认运行时会话后，SSH 监听随之关闭。
- 再次启动保留原主机密钥，密码登录仍成功。

同时修复了可执行段中数据被当作代码的问题。OpenSSL 的 `RC4_options` 返回位于可执行段的字符串，不能因为 RIP-relative LEA 指向该段便解码并修改它。现在只追踪已证明的间接调用目标、调用参数在被调函数中的调用，以及 musl 的线程清理回调注册；AOT 缓存版本更新为 33。

回归结果：指令补丁测试 21 项通过（另有 1 项性能测试未运行），代码入口测试 5 项通过，libc 正则测试 10 项通过，依赖安装工具测试 22 项通过。默认发行环境验收 12 项全部通过，包括清空 PATH、含空格路径、并发首次配置、默认清单启动、代理覆盖和退出码传递。

Codex 0.157.1 已用交互终端重新验证默认启动、登录选项方向键和 Ctrl+C，退出码 0；Claude Code 2.1.283 已通过联网检查、主题和登录菜单交互，最终退出码 0。未提供账户凭据，因此不包含登录后的模型请求验证。Claude 的联网测试显式传入了客体 HTTP/HTTPS 代理；首次 rootfs 下载仍自动使用清单规定的系统代理。

验证证据位于 `artifacts/sshd-default-20260927/`：`ssh-acceptance-final.json`、`release-validation.json`、各测试日志与交互记录。测试退出后已关闭 SSH 会话，并确认 Codex 测试 daemon 未运行。

候选包：`artifacts/sshd-default-release-assets/Kinakaze-0.1.0-preview-windows-x86_64.zip`，21,886,095 字节，SHA-256：

```text
ed99f739d738ab63ec728343ddcefc529695a645203aa3d193d1745dddeee9f4
```

ZIP 的 CRC 与全部已验证运行文件哈希一致；不含预装 rootfs、宿主 Python 或预生成 SSH 私钥。此包是本地候选包，未上传替换已发布版本。
