# 2026-09-22 软件运行边界实测

本轮固定使用 `artifacts/gnome-desktop-v79-dist`，没有把同时进行中的源码修改视为已发布能力。
通用工具使用 `artifacts/portable-dist/rootfs`；桌面软件使用 `artifacts/gnome-startup-root`。
测试期间 v79 的 36 个发布库文件哈希保持一致。既有 v72 桌面会话保持运行。

worker SHA-256：`f4cbb46c762a5e9da4a5879c6b79bfa9b5179b7670f9ccf29ec889325a5da67f`。
所有测试均运行实际 Linux 程序；宿主 Python 仅负责启动、测试文件生成、截图和结果收集。
窗口检查限定在本次测试的 Windows Job 内，使用独立 HOME、XDG 目录和 D-Bus 会话。

## 通用软件

`artifacts/software-boundary-v79-cli.json`：49 项中 **48 通过、1 失败**。
这些项目包含启动检查和功能检查，详细级别保留在 JSON 中。

- 通过：BusyBox/Bash、Python、Node、Java 版本启动、wget、SSH/sshd 启动、GCC/Clang 编译与运行、dpkg 包构造/安装/移除、APT 启动、SQLite 单进程事务、Redis 持久化、Nginx HTTP/TLS/Unix 上游、Git 本地及传输、Make/Ninja、rsync daemon、binutils/gcov、FFmpeg 音视频文件及管道。
- curl 在装载时失败：`libngtcp2_crypto_ossl.so.0` 引用的 `SSL_set_quic_tls_cbs@OPENSSL_3.5.0` 未解析。同一 root 配合旧 portable-dist，以及桌面 root 配合 v79 都复现。尚未定位具体依赖选择原因，不能从这一结果认定为 v79 新回归，也不能外推历史 HTTP/TLS 通过记录。

## 新增功能边界

入口：[software-boundary-matrix.json](../tests/guest/software-boundary-matrix.json)、[SoftwareBoundaryProbe.py](../tests/guest/SoftwareBoundaryProbe.py)。
`artifacts/software-boundary-v79-edge-final.json`：5 通过、2 失败、2 缺输入；两个失败均为 SQLite 场景。
桌面 root 的 bzip2 独立补测通过，证据为 `artifacts/software-boundary-v79-bzip2.json`。

| 软件 | 新增场景 | 结果 |
| --- | --- | --- |
| Git | 中文/空格文件名、1 MiB 二进制、空文件、真实合并冲突及 abort、fsck、禁用本地快捷方式的 clone、tar 导出校验 | 通过 |
| FFmpeg | 损坏 WAV、缺失文件、65×33 三帧视频的 FFV1 编码/解码逐字节比较、禁止覆盖已有文件 | 通过 |
| GCC / Clang | 非法 C、缺失库诊断、中文输出路径、程序输出和退出码 23 | 通过 |
| Node | 中文路径 1 MiB 二进制往返、子进程 stdout/stderr 各 256 KiB、退出码 23、ENOENT、4 个 worker threads | 通过 |
| gzip / tar | 中文/空格文件名、1 MiB 二进制、空文件、压缩往返与截断输入拒绝、归档提取内容比较 | 通过 |
| bzip2 | 中文/空格文件名、1 MiB 二进制、压缩往返与截断输入拒绝 | 在桌面 root 通过 |
| xz | 同类压缩边界 | 所测 root 缺少输入，未测试，未计通过 |
| SQLite | 连接持有写事务时另一进程读写 | 失败，详见下表 |

## SQLite 跨进程边界

[SqliteProcessBoundaryProbe.py](../tests/guest/SqliteProcessBoundaryProbe.py) 分别组合 ASCII/中文文件名、DELETE/WAL 模式、父连接有/无写事务、子进程读/写，共 16 个场景。
客体 SQLite 3.40.1 **6/16 通过**，重复执行复现；宿主 Windows SQLite 3.50.4 **16/16 通过**。
宿主对照验证探针预期行为，不是同版本 Linux 对照。

| 模式与场景 | 预期 | v79 实际结果 |
| --- | --- | --- |
| DELETE，无竞争的读写以及持有写事务时的读取，6 项 | 成功 | 通过 |
| DELETE，父连接持有 `BEGIN IMMEDIATE`，另一进程写入，2 项 | `SQLITE_BUSY` | 写入却成功；互斥边界失败 |
| WAL，跨进程读/写，有无竞争及两类文件名，8 项 | 读取/无竞争写入成功；竞争写入 BUSY | 全部返回扩展错误 4618 `SQLITE_IOERR_SHMOPEN` |

每个场景结束时 `integrity_check` 仍为 `ok`，本轮没有观察到数据损坏。
单进程 WAL/事务通过不能作为跨进程并发安全的证据。具体锁或共享内存实现根因仍待定位。
更旧 portable-dist 的对照在第一个子进程通信时即超时，没有得到可比较的 16 项结果，不能据此判断何时引入。

证据：

- `artifacts/software-boundary-v79-edge-final-logs/boundary-sqlite-process.stdout.log`
- `artifacts/software-boundary-sqlite-process-logs/sqlite-process-boundary.stdout.log`（首次独立分解）
- `artifacts/software-boundary-sqlite-host-control.log`
- `artifacts/software-boundary-sqlite-old-dist.json`

## 桌面应用与浏览器

入口：[test-software-boundaries.py](../tools/test-software-boundaries.py)。窗口测试检查首帧、等待内容绘制、文件名标题（适用时）、窗口截图、原生 WM_CLOSE 和所属命令退出。
`observed` 表示窗口观察和关闭条件通过；文件内容正确性仍须结合截图，不能直接当作完整应用兼容。
最终 22 项结果为 **14 项窗口观察完成、5 项功能检查通过、3 项浏览器检查失败**；
证据为 `artifacts/software-boundary-v79-desktop-final/results.json`。
整轮机器可读索引为 `artifacts/software-boundary-v79-summary.json`。

| 应用 | 场景及实测结果 |
| --- | --- |
| gedit / Mousepad | 中文/空格文件名、UTF-8、CRLF、无结尾换行、1 MiB 文本能显示并正常退出。截图中的中文为缺字方框，不能声称中文渲染通过。当前客体字体目录仅包含 DejaVu 的六个 TTF 文件。未测试编辑保存或输入法。 |
| EOG | PNG/JPEG/WebP 显示指定橙色矩形、绿色圆形及文字；正常关闭。45 字节截断 PNG 只显示灰色区域，仍可关闭；未观察到明确的损坏文件错误提示，不能计为正确拒绝损坏输入。 |
| Evince | 单页 PDF 显示预期文字和页数；损坏 PDF 显示明确错误提示，两者均正常关闭。 |
| File Roller | tar.gz/zip 列表可见；命令行提取后逐字节核对中文文本与空文件，均通过。 |
| Nautilus | 目录能显示。最初 8 秒关窗等待不足；独立复测约 12.1 秒后退出 0，窗口已消失。测试器默认关窗等待调整为 20 秒。 |
| GNOME Calculator | 窗口启动/关闭，命令行 `6*7=42`、`(2+3)^4=625` 通过。未据此声称图形按钮输入已验证。 |
| GJS | GI/GLib 读取中文路径文件和 JavaScript JSON 逻辑通过。 |
| Firefox | `--version` 即失败：`libxul.so` 缺 `snd_seq_close@ALSA_0.9`；同时有 user namespace clone EINVAL 日志。尚未到网页渲染。 |
| Chrome | 版本启动和本地 HTML headless 两项均在装载时缺 `xcb_parse_display`；尚未运行到浏览器 sandbox 或 DOM 检查。 |

多款 GTK 应用的日志还记录 portal 启动缺 `pw_core_get_client`。普通文件打开能完成，不代表文件选择 portal/屏幕采集已通过。
截图及逐应用日志在 `artifacts/software-boundary-v79-desktop/` 和 `artifacts/software-boundary-v79-desktop-final/`；首次超时及复测保留在 `artifacts/software-boundary-v79-recheck/`。

## 复现

在仓库根目录执行。测试器遇到失败或缺少程序返回非零，并保留原始结果。

```powershell
python tests/guest/tool-matrix.py --worker artifacts/gnome-desktop-v79-dist/worker.exe --root artifacts/portable-dist/rootfs --dist artifacts/gnome-desktop-v79-dist --manifest tests/guest/software-boundary-matrix.json --report artifacts/software-boundary-rerun.json --timeout 90
python tests/guest/tool-matrix.py --worker artifacts/gnome-desktop-v79-dist/worker.exe --root artifacts/gnome-startup-root --dist artifacts/gnome-desktop-v79-dist --manifest tests/guest/software-boundary-matrix.json --only boundary-bzip2 --report artifacts/software-boundary-bzip2-rerun.json
python tools/test-software-boundaries.py --root artifacts/gnome-startup-root --dist artifacts/gnome-desktop-v79-dist --output-dir artifacts/software-boundary-desktop-rerun --timeout 25
python tests/guest/SqliteProcessBoundaryProbe.py
```

最后一条为宿主 SQLite 对照，不经过 Kinakaze。桌面工具可用 `--only nautilus-directory` 等名称缩小范围。
49 项既有软件基线的本轮选择清单保留在 `artifacts/software-boundary-v79-cli-manifest.json`，可用现有 tool-matrix 的 `--manifest` 复跑。

本轮新增/修改的 Python 文件已通过语法检查，新增矩阵通过 JSON 校验，修改文件通过差异空白检查。
未修改或重建原生兼容库；上述失败作为可复现边界保留。
