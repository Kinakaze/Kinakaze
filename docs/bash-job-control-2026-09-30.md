# Bash setpgid 修复（2026-09-30）

修复管道组长快速退出、尚未被父进程回收时，后续子进程加入进程组被错误拒绝的问题。原实现用信号投递的“存活成员列表”判断进程组是否存在，排除了 zombie；此时 `setpgid` 返回 `EPERM`，Bash 可能打印 `child setpgid (...): Operation not permitted`。

运行库新增包含未回收进程的组/会话检查，供 `setpgid` 和终端前台组校验使用。信号投递仍使用可接收信号的成员列表。非子进程、已 exec 的子进程、会话组长及跨会话限制保留。Linux 的组成员检查可参见 [setpgid 实现](https://github.com/torvalds/linux/blob/master/kernel/sys.c)。

产物：`artifacts/bash-job-control-20260930/candidate`。使用更新的运行库重新启动会话后生效。

## 验证

- 旧产物的最小复现中，组长经 `waitid(WNOWAIT)` 确认退出但没有回收，父进程和后续子进程设置该组都返回 `EPERM`。
- 修复后，同一复现通过，父进程设置与子进程重复设置均成功。
- 真实 PTY 中执行 Bash：30 轮四段前台管道、10 轮三段后台管道，以及 Ctrl-C 中断前台管道并检查退出状态 130 和可用提示符，全部通过；转录中无 `child setpgid` 或 `Operation not permitted`。
- 进程组、原始 syscall、终端、跨进程信号/停止/继续共 36 项原生测试通过。本次两个 Rust 文件的格式检查与 `git diff --check` 通过。

原始证据位于 `artifacts/bash-job-control-20260930/`：`before/report.json`、`after/report.json`、`after/terminal.txt`、`native.json`、`build.log`。最小复现的失败记录保留；旧产物本轮短管道压力测试未随机触发报错，因此修复依据包括确定性的未回收组长回归。

```powershell
python tools/test-bash-job-control.py --root <guest-root> --dist artifacts/bash-job-control-20260930/candidate --output artifacts/bash-job-control-new
```
