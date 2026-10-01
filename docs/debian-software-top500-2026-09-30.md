# Debian 常用软件 500 项兼容性检查（2026-09-30）

本轮建立固定的 500 个软件包队列，按顺序检查实际可执行文件，再运行已注册的功能场景。缺包、缺少场景、超时和断言失败均保留在 500 项分母中。当前仍有覆盖缺口，不能据此声称 500 个软件全部兼容。

## 名单和统计口径

- 排序来源：[Debian popcon by_vote](https://popcon.debian.org/by_vote)。保留其 500,000 字节前缀及 SHA-256，足以选出本轮 500 项。
- 根据 bookworm main 的 `Contents-amd64.gz` 和 `Contents-all.gz`，筛选在 `/bin`、`/sbin`、`/usr/bin`、`/usr/sbin` 有命令入口，且已有 bookworm、bookworm-updates 或 bookworm-security APT 索引版本的包。最后一项的原始 popcon 排名是 2488。
- 这是筛选后的 500 个不同软件包；原始排名中的纯库包被排除，同一包的多个命令或别名占一个软件名额。`X11`、`mh` 等命令目录别名被排除。
- 名单、参考版本、下载路径和哈希位于 `config/debian-software-top500.lock.json`。CSV 分别记录 `locked_version` 和实际 `tested_version`，二者允许不同；例如安全更新可能使实际 OpenSSL 版本高于参考版本。
- Kinakaze 的适配基础包以 `kinakaze-base` 提供部分工具。CSV 的 `package_state` 标明这种来源，不能把它当作每个上游包都已独立完成安装配置。
- `passed` 只表示该包本轮所有**已注册场景**通过。未覆盖的其他命令仍在 `commands_without_scenarios` 中。版本或帮助查询只计入独立的启动统计。

## 最终结果

| 软件状态 | 数量 |
| --- | ---: |
| 注册功能场景全部通过 | 114 |
| 功能场景失败 | 14 |
| 功能场景超时 | 2 |
| 已有命令，但未注册功能场景 | 17 |
| 根目录中缺少该包的可执行命令 | 353 |
| 尚未检查 | 0 |
| 总计 | 500 |

共执行 574 条功能命令场景：525 条通过、47 条失败、2 条超时。软件通过率以固定 500 项计算为 22.8%，不能将缺包或未测项移出分母。另有 147 条启动检查，108 条退出码为 0、39 条为非零；它们不增加功能通过数。

Git、Nginx、Redis、MariaDB、FFmpeg、G++、Node.js 和 Ruby 的注册工作流通过；数据库场景包含事务、并发、持久化、重启或崩溃恢复，媒体场景验证实际编码解码结果。

最终聚合 JSON：`artifacts/debian-top500-20260930/verified-final/results.json`。完整顺序扫描来自 `verified-matrix-cs/results.json`；Fontconfig 单项复测来自 `fontconfig-recheck/results.json`。原用例错误地把模式中的连字符当作普通字体名，更换为有效模式后通过。聚合保留来源报告的 SHA-256 和每行来源编号，并校验两轮的根目录、运行库、dpkg 状态和可执行文件清单一致；不混合不同运行库版本的结果。

逐项结果：`docs/debian-software-top500-results.csv`。原始 JSON、每条命令的脚本、退出码、超时标志、实际二进制哈希、输出及日志目录：`artifacts/debian-top500-20260930/verified-matrix-cs/`。

运行库：`artifacts/debian-top500-20260930/candidate-r2/`。测试根目录：`artifacts/debian-top500-20260930/root-cs/`。所有安装、账户和数据库操作都发生在这个临时根目录中。

## 已补上的缺口

1. **跨 PID 优先级**：`getpriority/setpriority(PRIO_PROCESS, pid)` 能查询和修改真实目标进程。解析 guest PID 后核对 Windows 进程创建标识，持有目标句柄，并检查 UID 和 `CAP_SYS_NICE` 权限。回归验证目标变化、调用者不变、子进程自行观察到变化、已回收 PID 返回 `ESRCH`，以及其他 UID 修改目标时返回 `EPERM`。
2. **scanf 动态分配**：实现窄字符 `%ms`、`%m[`、`%mc`，包括宽度、抑制赋值、终止符和错误返回；修复扫描集初始不匹配被误报为 EOF 的问题。真实 C 程序覆盖 `sscanf/fscanf`、指针赋值和 1000 次分配释放；psmisc 的 `prtstat` 使用该能力解析进程状态。
3. **测试根目录的大小写冲突**：普通复制丢失 NTFS 大小写敏感属性，且 robocopy 的名称匹配也会合并大小写不同的路径。新增 `tools/clone-debian-root.py`：先标记目录，再按精确文件名复制，保留 Linux EA 中的权限和虚拟软链接元数据。真实 NTFS 测试验证 `HEAD/head`、`Pod/pod` 分别保留；测试入口拒绝关键目录大小写不敏感的根目录。
4. **场景和数据准备**：补充编译链接、Git、SQLite、rsync、Make、Patch、XML、Node、Ruby、MIME、Fontconfig、socat、DBI、Redis CLI、MariaDB 辅助工具等真实工作流。修正 `pwd/pwdx/lsof` 固定目录断言、`invoke-rc.d` 参数和发行版身份断言。

真实 APT 安装补充了 sudo、jq、rsync、bc、sqlite3、gawk、make、patch、G++、XML 工具、pkgconf、dc、Perl 文档和依赖、FFmpeg、MariaDB。FFmpeg 首次安装超时导致 dpkg 中断，随后通过 `dpkg --configure -a` 恢复，FFmpeg 和 MariaDB 的重试安装均成功；`apt-get check` 和 `dpkg --audit` 通过。首次超时及恢复日志保留，不能把安装重试时间当作冷启动性能对比。

早期 `final-matrix/`、`verified-matrix-r2/` 使用过存在大小写冲突的复制根目录，只作诊断历史；最终结论使用 `verified-matrix-cs/`。对应旧用例代码保留在 `matrix-source/` 和 `verified-matrix-r2/sources/`。

## 验证和剩余工作

- 正确保留大小写的根目录中，优先级和 scanf 回归各连续通过三轮，记录在 `fixes-cs/`。
- libc 的现有优先级 Windows 调度类测试通过，记录在 `native-priority-r2.json`。没有声称运行了整个 workspace 测试套件。
- 5 项队列与报告测试通过，其中包括真实 NTFS 大小写复制测试，以及拒绝合并不同运行库结果的测试；新增 Python 文件的 Ruff 检查通过。
- NativeOnly release 构建和导出检查通过；最终单独的 `cargo fmt --all -- --check` 通过。构建包含工作区中其他已有改动，本报告只归因上述实际修复。
- 优先级仍使用原有 Windows 调度类映射，尚不支持全部 Linux nice 精度及非零进程组/用户目标；scanf 本轮只实现窄字符 `%m`。
- systemd 服务总线、设备接口、部分 netlink、调度和 capability 操作等失败继续保留。GUI、内核或硬件依赖的软件需要相应环境和场景；缺包的软件尚未运行其功能测试。
- Coreutils 的 `install /dev/null ...` 仍出现源文件身份检查错误，另有诊断记录；空登录日志的数据准备使用正常文件创建完成，未将这次错误判为通过。

## 顺序复测

从已经准备好的根目录复制到新目录；复制工具跳过历史 `/tmp`、`/var/tmp` 测试内容，保留软件和 dpkg 状态：

```powershell
python tools/clone-debian-root.py --source artifacts/debian-top500-20260930/root-cs --root artifacts/debian-top500-next/root
python tools/test-debian-software.py --root artifacts/debian-top500-next/root --dist artifacts/debian-top500-20260930/candidate-r2 --output artifacts/debian-top500-next/results --startup --stop-on-gap
```

同一根目录、运行库、名单、用例、dpkg 状态和命令内容没有变化时，可用 `--resume` 跳过已通过的软件；`--only psmisc,bsdutils` 可限制到指定包。修改运行库、用例、软件版本或可执行文件后使用新的输出目录，保留先前失败记录。默认运行每个包的全部注册命令场景，不以 `--help` 补齐功能通过数。聚合 JSON 只用于报告，不作为 `--resume` 的执行检查点。

```powershell
python tools/merge-debian-software.py --results artifacts/debian-top500-20260930/verified-matrix-cs/results.json --results artifacts/debian-top500-20260930/fontconfig-recheck/results.json --output artifacts/debian-top500-20260930/verified-final/results.json
python tools/report-debian-software.py --results artifacts/debian-top500-20260930/verified-final/results.json --csv docs/debian-software-top500-results.csv
```
