# Debian 预装命令批量实测（2026-09-26）

已对 **758 个命令入口中的 739 个**做启动或功能检查。其中 **514 个入口（67.8%）执行了实际功能场景，420 个通过、90 个失败、4 个超时**。这些结果不能解释为完整 Debian 兼容性，也不能将帮助页输出视为功能通过。

测试对象为 `artifacts/debian-standard-proc-stat` 的 Windows x86_64 本地预览构建，基于 `926f54ba3ac381dcabaf2bd263909249eaaa0aaa`。结果对应下面锁定的包内镜像，不代表当前工作树的其他改动或已经发布的 v0.1.0。

| 镜像 | SHA-256 |
| --- | --- |
| worker.exe | `b4aabb59dcb156f06b545e778b66cb76f68e079fc076053d8a512f8ce8e40a84` |
| rootfs.manifest.json | `1c2dc793d4e435130740bf8a59d6685e91c57e7f73772a97660ccb91ce9a5abf` |

## 覆盖口径

枚举 manifest 中 `/bin`、`/sbin`、`/usr/bin`、`/usr/sbin` 的直接文件和链接入口，共 653 个文件入口、105 个链接入口，解析后为 657 个不同目标路径。别名分别执行并计数；758 不是独立 ELF 二进制数量，更不是 Debian 软件仓库的所有命令。

| 检查 | 通过 | 仅返回用法 | 失败 | 超时 | 未执行 | 总数 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 启动、帮助或版本 | 515 | 140 | 75 | 0 | 28 | 758 |
| 实际功能场景 | 420 | 不适用 | 90 | 4 | 244 | 758 |

“仅返回用法”表示程序返回非零退出码并输出用法，证明执行到参数处理阶段。功能通过必须同时满足场景的退出码约定和内容、文件、元数据等断言；`false`、未匹配查询等预期非零状态有单独断言。两种检查不能相加：实际执行过至少一种检查的入口为 739 个，两种都未执行的是 19 个。

每个功能结果只覆盖 CSV 中说明的场景。有些场景包含其他命令，例如 `stty` 使用 `script` 创建伪终端；场景失败不等于已经定位到被测命令本身。错误分类来自日志匹配，是排查线索，不是根因分析结论。

## 已通过的典型场景

- Shell 变量、算术、管道，以及文件复制、改名、权限、所有者、硬链接、查找、文本处理和摘要校验。
- gzip、bzip2、xz、zstd、tar、cpio、ZIP 的压缩、解压或往返校验；Vim/vi 批量编辑；分页与文档格式处理。
- Python 导入 `ssl`、`sqlite3`、`json` 并执行 SQLite 查询；Perl 模块、JSON、POD、TAP 等场景。
- APT 已安装列表、依赖一致性和配置查询；`dpkg` 在独立目录安装自制 `.deb`，核对有效载荷后卸载；`dpkg-deb` 读取包元数据。
- curl、wget、netcat 对受控本地 HTTP 服务的请求；SSH 配置展开、密钥生成和临时 agent；OpenSSL 摘要与随机数据。
- `getent passwd root`、`ip -j addr show`、`lsof` 工作目录查询，以及 ext、Minix、cramfs 和 swap 的独立普通文件镜像操作。

上述 APT 检查不覆盖公网仓库更新、任意真实软件包安装和上游维护脚本全集；SSH 检查不覆盖远端登录和 SFTP 传输。nano、默认 editor 的交互编辑等仍未做功能验证，可在清单中查看 `not_tested`。

## 实测缺口

| 影响范围 | 观察结果 |
| --- | --- |
| 装载符号 | 启动检查有 59 个入口报缺失符号。`fgetsgent@GLIBC_2.10` 阻塞 34 个入口；此外包括 `sigqueue`、`wordfree`、`dn_comp`、`nearbyintl`、`iopl`、`getttynam` 及 GLIBC_PRIVATE 入口。完整符号和受影响路径见 `summary.json`。 |
| 数值结果 | `numfmt --from=iec 2K` 返回成功但输出 `0`，期望 `2048`；内容断言正确捕获了问题。 |
| DNS 工具 | `dig`、`mdig`、`host`、`nslookup` 在受控回环 DNS 查询中以 143 退出；部分查询已有响应仍未正常退出。单线程复测仍失败。 |
| 终端场景 | `script` 在重定向输入的批处理场景中没有捕获到子命令输出，关联的 `stty`、`tty` 场景也失败；不据此推断所有交互终端功能都不可用。 |
| 文件系统、进程元数据 | `df` 报 Invalid argument；`prtstat` 不能解析 stat，`pmap` 未得到预期映射；`renice`、`getpcaps` 对当前 shell 的 PID 查询失败。 |
| 内核接口 | `/proc/modules`、部分 `/sys` 设备视图、generic/RDMA/devlink netlink 等缺失或不支持；`ionice`、`uclampset` 返回未实现。 |
| 默认配置与包状态 | `ucf` 自检认为自身未安装，`chage` 缺 shadow 文件；`ldconfig` 缺默认缓存，`whatis/apropos` 未找到索引；`perldoc`、`ptardiff` 缺相应文档包或模块。不能把仅有文件和 Provides 视为上游软件包已完整配置。 |
| 系统服务和权限 | `systemctl` 返回 offline；`su/runuser`、文件 capabilities 等场景失败。预装 systemd 工具不代表已经运行 systemd 服务体系。 |
| 故障与超时 | `sg` 有 guest fault；`lscpu` 有 `0xc0000096` 特权指令异常。提高上限并单线程复测后，`dnsdomainname`、`capsh`、`lscpu`、`tasksel` 仍在 30 秒超时。 |

优先修复会返回错误结果的数值和终端路径、DNS 异常退出，再补覆盖面较大的公共符号与包状态；设备或完整服务体系的缺口另行界定支持范围。此轮只交付验证和缺陷清单，没有用空实现掩盖失败，也没有据此重发正式 release。

## 可复跑方式与证据

```powershell
python tools/test-debian-commands.py --dist artifacts/debian-standard-proc-stat --output artifacts/debian-command-coverage --phase smoke --jobs 6
python tools/test-debian-commands.py --dist artifacts/debian-standard-proc-stat --output artifacts/debian-command-coverage --phase functional --jobs 6
python tools/test-debian-commands.py --dist artifacts/debian-standard-proc-stat --output artifacts/debian-command-coverage --phase functional --jobs 1 --timeout 30 --merge --only '/(dig|mdig|host|nslookup|numfmt|script|stty|tty|capsh|dnsdomainname|toe|ucf|tasksel|df|sg|lscpu|dpkg)$'
python tools/report-debian-commands.py --results artifacts/debian-command-coverage --csv docs/debian-command-results.csv
```

使用新的 `--output` 目录会为每种检查分别创建首次初始化的临时 rootfs，避免启动扫描改变功能测试的默认状态。每个用例还有独立工作目录，磁盘测试只操作普通文件镜像，账户修改在私有 prefix 内进行。宿主生成的夹具在 guest 中显式设置权限；成功场景不依赖宿主文件的隐含模式。

默认单条上限 12 秒；复测可用 `--timeout` 提高上限。`--only` 选择入口，`--merge` 保留未选择入口的已有结果。每轮日志使用单独时间戳保存；进程先以挂起方式创建，再加入 Windows kill-on-close Job，超时或结束会清理该次启动的子进程树。HTTP、DNS 功能用例使用本机动态端口，不依赖公网服务。

- [逐命令结果 CSV](debian-command-results.csv)：全部 758 行，含别名目标、两种状态、功能场景、失败摘录和日志路径。
- [批测器](../tools/test-debian-commands.py)、[功能场景](../tools/debian_command_cases.py)、[报告生成器](../tools/report-debian-commands.py)。
- 本地原始记录：`artifacts/debian-command-coverage/{inventory,smoke,functional,summary}.json`，逐轮 JSONL，以及 `smoke/`、`functional/` 下的输入和输出文件。
- 本地证据归档：`artifacts/debian-command-coverage-2026-09-26.zip`。原始 JSON 包含实际参数、退出码、耗时和断言相关夹具输出；CSV 省略成功命令的大量帮助文本。

两类检查均未执行的 19 个入口：`dhclient-script`、`e2scrub_all`、`installkernel`、`killall5`、`mkhomedir_helper`、`pam_namespace_helper`、`pwhistory_helper`、`shadowconfig`、`unix_chkpwd`、`unix_update`、`bashbug`、`clear_console`、`debconf-apt-progress`、`sensible-browser`、`ssh-argv0`、`exicyclog`、`exiwhat`、`ownership`、`update-pciids`。这些辅助入口需要专用的调用上下文，未用通用未知参数把它们算作通过。

脚本通过 Ruff 检查、格式检查与 Python 编译检查。失败、超时和未测项均保留，当前结果尚不足以宣称“大部分 Debian 日常功能都已兼容”。
