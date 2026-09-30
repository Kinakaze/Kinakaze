# utmp 文件锁与定位修复（2026-09-30）

针对 utmp 的 `fcntl` / 定位检查返回 `EINVAL` 的反馈，使用独立的临时 utmp 文件复现并修复了以下缺口。测试没有修改系统正在使用的 utmp/wtmp。

## 复现与修改

- 原有普通进程锁 `F_GETLK/F_SETLK/F_SETLKW` 的正负区间、`SEEK_CUR/SEEK_END` 在普通文件上通过检查；OFD 命令 `36/37/38` 则统一返回 `EINVAL`。现在支持 `F_OFD_GETLK/F_OFD_SETLK/F_OFD_SETLKW`，OFD 与进程锁之间也执行真实冲突检查。OFD 请求仍要求 `l_pid == 0`；查询到 OFD 冲突时返回 `l_pid == -1`。
- OFD 锁使用已有共享文件描述的生命周期标识，随 `dup`、`fork`、`exec` 和 `SCM_RIGHTS` 传递；独立 `open` 不共享所有权。最后一个引用关闭后释放锁，进程退出也不会留下永久锁。正常关闭/解锁通过事件唤醒；OFD 持有者异常退出采用最多 50 ms 间隔检查共享对象是否消失。
- tmpfs 文件原先在锁操作中返回 `EBADF`。现在按 tmpfs 的卷与 inode 身份纳入同一个锁注册表，采用实际共享位置与逻辑文件长度解释相对区间。普通进程锁仍遵守“关闭同 inode 的任意描述符即释放该进程的锁”的规则。
- 普通文件现在支持 `SEEK_DATA/SEEK_HOLE`：采用 Linux 允许的保守行为，将 EOF 之前视为数据、EOF 视为洞。越过或到达 EOF 返回 `ENXIO`，负偏移返回 `EINVAL`，失败保持原文件位置。tmpfs 保留已有按页查找洞的实现。
- `pututxline` 原先通过宿主接口读出并重写整个文件，既绕过 guest 的 `fcntl` 锁，也存在并发更新丢失的风险。现在通过 VFS 打开、加锁、定位并写入单条记录；读取和 `updwtmpx` 也经过同一锁机制。进程内另用互斥锁串行化操作，因为 POSIX 记录锁不会阻挡同一进程的线程。
- utmp 的目录创建和路径访问全部经过 VFS，消除了 tmpfs 路径无法转换为宿主路径而产生的另一个 `EINVAL`。

实现位于 `libs/libc/src/fdio.rs`、`libs/libc/src/sysdb.rs`、`engine/crates/kinakaze-vfs/src/record_lock.rs` 和 `engine/crates/kinakaze-vfs/src/fs.rs`。共享锁表名称升级为 v3，避免新旧布局相互解释；使用新构建启动会话。

## 验证

- 原生回归 **14 项通过**：utmp 读写/遍历、记录锁区间拆分与合并、死锁检测、信号中断与重启、异常事务恢复、持锁进程死亡、OFD 别名/独立打开/混合锁冲突、最后关闭唤醒、定位失败保持偏移。
- guest 回归 **21 组通过**：在 `/tmp`、`/run`、`/dev/shm` 分别运行 7 组检查。锁矩阵同时调用 libc `fcntl` 和原始 syscall；每个目录检查 72 组有效命令/区间组合，并验证非法 PID、区间、溢出和 EOF 错误。
- 每个目录的 utmp 流程验证外部进程持锁时 `pututxline` 确实等待，随后 4 个进程各追加 8 条记录，最后替换已有槽位。验证 utmp 为 33 条唯一记录、wtmp 为 32 条记录，文件长度准确；已安装的 `utmpdump`、`who`、`last` 读取私有测试文件成功。
- 额外的 `UnixRightsProbe` 通过，检查已有 Unix 描述符传递功能未被破坏。
- Release 构建成功，包含 29 个原生模块；本次修改的 Rust 文件格式检查和 `git diff --check` 通过。收尾时全工作区 `cargo fmt --all -- --check` 被另一路正在修改的 `apps/init/src/image_cache.rs` 与 `crates/protocol/src/lib.rs` 格式差异阻挡，没有改写这些并行工作文件。

旧版 18 组初始测试中只有两组普通文件进程锁测试通过，其余复现 OFD `EINVAL`、tmpfs 锁 `EBADF` 或 utmp 绕过锁。第一版修复通过 20/21 组，剩余 tmpfs utmp 写入仍报 `EINVAL`，进一步定位并移除了宿主路径转换。原始失败日志与最终通过日志均保留。

产物与原始记录位于 `artifacts/utmp-locks-20260930/`：

- `candidate-final/`：完成验证的构建。
- `before/UtmpLocksProbe.log`：旧版失败记录。
- `after-r1-expanded/UtmpLocksProbe.log`：tmpfs 路径问题的隔离记录。
- `after-final/UtmpLocksProbe.log`、`after-final/UnixRightsProbe.log`：最终 guest 回归。
- `native.json`、`native-final.log`：原生测试。
- `build-final.log`、`verification.json`：构建与汇总检查。

复现命令：

```powershell
python tools/test-runtime-compatibility.py `
  --root artifacts/performance-20260927/common-root `
  --dist artifacts/utmp-locks-20260930/candidate-final `
  --probe UtmpLocksProbe --probe UnixRightsProbe `
  --output-dir artifacts/utmp-locks-20260930/recheck --timeout 120
```

依据：[Linux fcntl 记录锁语义](https://man7.org/linux/man-pages/man2/fcntl_locking.2.html)、[Linux lseek 语义](https://man7.org/linux/man-pages/man2/lseek.2.html)、[Linux 内核锁实现](https://github.com/torvalds/linux/blob/master/fs/locks.c)。本次覆盖上述回归场景，并不表示所有 utmp、文件系统与锁的组合已经穷尽。
