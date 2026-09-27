# GNOME 第五轮：IBus/XIM 隐藏失败与启动开销（2026-09-26）

深入跟踪找到了实际兼容性缺陷：原生 libX11 缺少 XIM 使用的 `_XRegisterFilterByType` / `_XUnregisterFilter`，而 `XFilterEvent` 原本始终返回 False。补全后，真实 IBus XIM 连接握手通过，启动期间累计创建的 Windows 进程从 67 个降到 61 个。启动中位数本批为 4.23 → 4.13 秒，宿主负载波动使这个差值不足以证明稳定提速。

保留这项功能修复；额外的“由启动器直接启动 XIM”方案没有合入。生产 Python 启动器仍与第三轮版本逐字节相同。本轮修改在原生 libX11，需要重建分发才能生效，旧分发不会随源码修改自动更新。

## 为什么先前的 IBus 检查没有发现它

现有检查覆盖 session bus 名称、Shell 的 IBus panel 和 CreateInputContext；XIM 是另一个子进程和协议，daemon 的总线已就绪不等于 XIM 可用。

给 IBus 增加 verbose 日志仍不能直接看到 XIM 装载错误，因为 `execute_cmdline()` 默认把子进程 stdout/stderr 重定向到 `/dev/null`。本轮只修改测试根 `/tmp` 下的一份 IBus ELF 副本，让它继承这两个描述符；原 `/usr/bin/ibus-daemon` 没有改动。副本有原文件摘要、原字节核对与偏移记录，存于 `diagnostic-elf.json`。

可重复观察到的路径为：

1. daemon 请求执行 `/usr/libexec/ibus-x11 --kill-daemon`。
2. 装载器因 `_XUnregisterFilter` 无法解析而失败。
3. GLib 对 ENOEXEC 的回退尝试 `/bin/sh /usr/libexec/ibus-x11 ...`，二进制内容被当成脚本执行，随后产生无效命令和语法错误。
4. `exit-repro-10` 捕获到一次该回退执行返回 EIO，IBus 报 `Failed to execute child process` 并退出，启动器观测到 255。

这解释了此前隐藏的无用进程，以及一次具体的 IBus 退出链路。通用 exec 失败恢复中为什么回退进一步返回 EIO，仍有运行库层面的待查问题；本轮消除的是正常 XIM 启动触发这条失败路径的原因，不能宣称修复了所有 exec 失败恢复问题。

修复后的 `fixed-xim-trace` 只看到正常执行 ibus-x11，没有上述 `/bin/sh` 回退，并收到真实 `XIM_XCONNECT_OK`。

## 实现与验收

`libs/libX11/src/event_filter.rs` 实现按 Display、窗口和事件类型范围匹配的过滤器，最新注册者优先，返回第一个匹配回调的结果（包括 False）。回调在释放锁后调用，允许自注销；注销删除所有精确匹配的注册。关闭 Display 清理其过滤器。

新增 fork participant 保存注册顺序和客体回调/数据指针，使用可序列化字段恢复，避免把原生 Vec 或 Mutex 直接搬到子进程。共享 Display 使用可重绑定标识，本地窗口保存逻辑标识。当前真实 fork 探针覆盖 root 窗口过滤器、客体回调和栈上数据；没有据此声称任意应用窗口的 fork 行为均已验收。

接口行为与缓存中已锁定的 Debian `libx11-6_1.8.4-2+deb12u2_amd64.deb` 内真实 libX11 的反汇编核对。参考 ELF 的 SHA-256 为 `d88c973e79fd9b65838d77624142952757e47a6eb1a58602acf0911cf35989f4`。该 ELF 只用于核对，未替换原生 provider。

验证结果：

- 四项 Rust 测试通过：类型边界、Display/窗口匹配及覆盖窗口，最新过滤器返回 False，自注销/重复注销，Display 清理。
- `tests/guest/XimFilterProbe.c` 经真实客体 fork 验证：子进程继承回调，子进程注销及数据写入不影响父进程；父子均能调用并注销过滤器。两次受控探针均通过。
- `tests/guest/GnomeXimProbe.py` 查找 `@server=ibus` selection，发送 `_XIM_XCONNECT`，收到服务端回复。它检验真实服务与事件过滤，不只是检查导出符号或进程存活。
- 真实 Shell 与应用同时运行时，私有总线、自动激活服务环境、IBus panel、输入上下文、XIM 握手均通过。
- 故障注入仍正确失败：IBus 提前退出不进入后续探针；会话探针退出 7，报告保留 exit_code 7，宿主启动器返回失败。
- 当前工作区 `cargo check -p kinakaze-v2-libx11 --locked --target-dir target/workspace-check` 通过，新增 Rust 模块格式检查通过。

原生构建使用第四轮的冻结源码背景，只加本轮 libX11 修复，产物为 `artifacts/gnome-speed-round5-20260926/candidate-dist`。原有基线分发保持原样，避免混入工作区其他并行改动。Rust 测试依赖另行按完整 workspace 的一致 feature 图构建和打包，没有把测试 DLL 混入候选分发。

## 18 次交替对照

六组顺序为 ABC、BCA、CAB、CBA、BAC、ACB，使每个方案在三个位置各出现两次。A 为原基线；B 为修复 libX11 后保持原启动方式；C 在 B 基础上让 Python 直接执行同一个 ibus-x11，避免 daemon 的一次中间 fork。每次新建会话，关闭诊断日志和额外探针，Shell 就绪后观察 1 秒。

| 指标（成功样本中位数） | A 原版 | B 修复 XIM | C 直接启动 XIM（实验） |
| --- | ---: | ---: | ---: |
| 成功启动 | 6/6 | 6/6 | 6/6 |
| Shell startup-complete | 4.228 秒 | 4.131 秒 | 3.847 秒 |
| 最短～最长 | 3.491～5.353 秒 | 3.556～5.310 秒 | 3.725～6.309 秒 |
| Shell exec 之前 | 1.448 秒 | 1.466 秒 | 1.412 秒 |
| IBus 启动至总线就绪 | 0.683 秒 | 0.694 秒 | 0.429 秒 |
| 总线就绪至 Shell exec | < 日志采样分辨率 | < 日志采样分辨率 | 0.204 秒 |
| 累计 Windows 进程数 | 67 | 61 | 60 |
| 整次 Job CPU 时间 | 13.406 秒 | 13.461 秒 | 12.117 秒 |

CPU 时间为多进程、多核累计，不是启动墙钟时间；包括就绪后的观察和退出。宿主 CPU 忙碌比例在各次测量中约为 22%～56%，包含测试自身开销，不能直接当作外部负载。长尾期间多个启动阶段同时变慢。没有丢弃长尾或失败来挑选成绩，也不能把本批数字与第四轮 3.426 秒直接计算回退/提速。

A 的 XIM 实际失败，B 的 XIM 正常运行，两者的功能覆盖不同。本轮主要结论是在相近启动耗时下恢复了 XIM，不能只用速度数字替代功能验收。

C 的 IBus 等待确实更短，但其中约 0.20 秒转移到了总线就绪之后；前置路径中位数只比 B 少约 55 毫秒。完整启动的差异又混有后半段和宿主调度波动。本批不足以确认稳定收益，因此没有改变生产启动器的 XIM 生命周期管理。

## 还剩在哪里

B 的阶段中位数为：Shell 前置启动 **1.466 秒**，Shell exec 至 Mutter 日志 **0.532 秒**，Mutter 至 Shell 就绪 **2.132 秒**。前置阶段中 IBus 等待占 **0.694 秒**。各段分别取中位数，不要求相加等于整体中位数。

更大的剩余区间仍在 Mutter/GJS/UI 初始化。第四轮已经指出 Mutter 至首个 JS 标记的未细分区间，以及模块导入、Panel、背景布局等开销；本轮没有获得新证据把它们归因于某一个函数。进一步压缩应在原生/GJS 边界加计时或 CPU 采样；仅缩短 IBus 轮询间隔、挪动 ready 标记或停用输入组件，没有稳定收益依据。

## 工件和测试边界

本轮共 **40 次受控运行**，包括诊断、故障注入和性能对照。全部在独立非输入 Windows 桌面，没有 SwitchDesktop；内层期限 24 秒，外层独立 Job watchdog 27 秒。最长运行及进程清理 **7.634 秒以内**，40 次清理后的活动进程数全部为 **0**。

固定同一个测试根、时区、locale、缓存和偏好；测试根的动画关闭设置沿用第一轮，生产启动器没有新增关闭动画行为。文件缓存已热且未受控清空。终点仍为宿主收到 Shell startup-complete 日志，约有 20 毫秒轮询及调度误差。XIM 握手不等于物理键盘输入或应用内中文组词验收，原有完整 GNOME 服务边界仍适用。

`artifacts/gnome-speed-round5-20260926/analysis.json` 保存所有运行、阶段统计、清理核对和修改文件摘要。每个试验目录包含完整 stdout/stderr；`compare.py` 与 `analyze.py` 可复现统计。使用新 tag 串行运行受控测试：

```powershell
python artifacts/gnome-speed-round5-20260926/run.py --tag new-fixed --dist artifacts/gnome-speed-round5-20260926/candidate-dist
python artifacts/gnome-speed-round5-20260926/run.py --tag new-integration --dist artifacts/gnome-speed-round5-20260926/candidate-dist --app-probe --ibus-probe --xim-probe --hold 3
```

第二条依赖测试根 `/tmp/GnomeXimProbe.py` 已由仓库同名探针复制；测试驱动为此现场准备，不是通用安装脚本。C fork 探针用 `clang --target=x86_64-linux-gnu -fuse-ld=lld -shared -fPIC -O2 -nostdlib` 编译，链接基线 `rootfs/usr/lib/x86_64-linux-gnu` 中的 `-lX11 -lc`，输出到测试根 `/tmp/xim-filter-probe.so`。`xim-probe-launcher.py` 调用其入口后执行 XIM 握手；用上述受控驱动的 `--launcher` 选择它即可。
