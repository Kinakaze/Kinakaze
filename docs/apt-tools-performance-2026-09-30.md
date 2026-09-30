# APT 本地安装与开发工具第三轮验证

本轮继续优化本地文件和元数据操作，补全离线镜像中的编译链接接口。
候选产物为 `artifacts/apt-tools-round3-20260930/candidate/`，完整离线镜像为
同目录的 `offline-image-final/`。没有覆盖用户正在运行的发行目录。

## 实现

- inode 事务新建命名互斥锁时直接取得所有权，省去无竞争路径的一次
  `WaitForSingleObject` 内核调用。已有对象仍通过原来的递归加锁、遗弃锁
  接管和可中断等待路径取得所有权。每个事务结束仍释放并关闭句柄，没有
  新增常驻缓存、后台队列或跨 fork 的进程内锁表。
- `prepare-release-rootfs.py` 将已有的 ELF 链接接口安装逻辑扩展到离线镜像。
  原生 PE provider 仍保留在 `/lib`；ELF 符号接口进入 Debian 多架构目录。
- 如果镜像包含 libc 开发链接脚本，将其中的 loader 链接输入指向单独的
  ELF 接口，保留 `/lib64/ld-linux-x86-64.so.2` 的原生命令功能。未修正的
  脚本可以链接简单程序，却会让不依赖 libc 的叶子共享库混入错误的输入，
  后续 GNU ld 报 `file format not recognized`。实际共享库和解释器命令
  回归覆盖了这个区别。
- 工具回归新增本地 Git 分支、提交、合并、gc、fsck、clone、archive，
  以及 tar/gzip/xz/zip/unzip 的二进制往返校验。Cargo 夹具显式选择已安装
  的 GCC，不依赖旧测试根中缺失的 `cc` alternatives。Bash 夹具启用
  `pipefail`，使管道前端失败也能传回测试结果。

新锁所有权的依据是 Microsoft 的
[CreateMutexW 文档](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createmutexw)：
初始所有权只适用于新创建的对象；`ERROR_ALREADY_EXISTS` 表示仍须取得已有锁。

## 本地安装性能

基线为上一轮 `artifacts/apt-local-round2-20260930/candidate/`。同一根目录，
每轮生成独立包数据库和目标目录，512 个 4 KiB 文件、12 个维护脚本子进程，
使用本地 `.deb`，不计网络下载。两个版本按 ABBAAB 顺序各测三次。
真实安装、编译和构建任务完成后重新测量；以下采用最终一组数据。

| 阶段 | 基线中位数 | 候选中位数 | 时间变化 |
| --- | ---: | ---: | ---: |
| 本地 APT 安装 | 6.014 秒 | 4.796 秒 | **−20.3%** |
| dpkg 解包 | 3.996 秒 | 2.710 秒 | **−32.2%** |
| 配置维护脚本 | 0.891 秒 | 0.907 秒 | +1.8% |

六轮的各阶段及文件内容验证全部通过。配置脚本阶段没有可确认的收益，
本轮改善主要在解包路径。

原始报告：`final-balanced-results.json`、`final-comparison.json`，均位于
`artifacts/apt-tools-round3-20260930/`。每个报告保留实际发行文件 SHA-256、
阶段结果和安装内容断言。早期一组在后台安装任务运行时测得本地 APT
5.924 → 4.671 秒、解包 3.812 → 2.720 秒，保留在 `comparison.json`，
不与最终数据混用。

候选同时包含工作区同期的路径查询、EA、原子创建和 fork 优化，整包差值
不能全部归因于本轮消除的单次锁等待。这是同机测试包的结果，不是所有
软件包、所有设备或原生 Linux 的速度保证。维护脚本、触发器、fsync 和
文件内容校验均保留。

## 真实包安装与兼容性

- 新根中安装 Debian Node.js/npm 及 **357 个新增包**，本地安装阶段
  **402.588 秒**。下载独立完成后运行 `apt-get --no-download install`。
  Node JavaScript/crypto、npm、`dpkg --audit`、`dpkg --verify` 全部通过。
  这是完整性验证的一次实测；该期间并行运行了兼容性/镜像准备工作，
  不把它与上一轮单次时间相减后宣称严格的性能提升。
- **19 类工作负载各两轮，共 38 项通过**：Shell、Python、Python 多进程、
  GCC、Clang、C++ future、Cargo 离线测试及增量重建、CMake/Ninja/CTest、
  Python venv/pip、npm pretest/test、Node、Ruby、PHP、Git、归档压缩，
  以及 fork 后 dlopen、deepbind、解释器命令和可执行 TLS。
- 最终脚本修复后再次运行 deepbind 和解释器命令，各两轮通过。
  `toolchain-unmodified-scripts/` 保留撤去脚本修复后两轮重现的失败；
  `common-verified/` 和 `linker-final/` 保存修复后的结果。
- zcode 完成 **12/12** 次真实工具操作闭环，含读取、编辑、失败/成功测试、
  stderr 与大输出。使用本地确定性协议服务，不代表在线账号或模型服务验证。
- 重新生成完整离线镜像：**16,134 个文件、3,128 个链接、306 个基础组件**。
  该全新根的 APT 数据库、Shell、Python、目录类型、元数据路径、路径引用
  和 xattr 生命周期 **7 项通过**。开发工具测试使用补齐接口的既有测试根；
  不声称这 19 类开发工具均已预装在基础镜像中。
- **52 项原生测试通过，3 项既有忽略**：inode 锁递归/竞争/中断、跨进程
  xattr、fs-verity 事务、创建与所有权、writeback。
- **23 项 Python 包准备测试通过**，包含在线和离线 manifest 的 ELF 输入、
  原生 provider 和可执行 loader 的区分。格式检查及 `git diff --check` 通过。

日志与完整结果还包括 `native.json`、`package-tests-final.log`、
`node-candidate/report.json`、`offline-verified/results.json`、`zcode/report.json`、
`build.log`、`prepare-offline-final.log`。首次 Node 根准备曾返回宿主
`ERROR_ACCESS_DENIED`，留下的失败日志为 `setup-node.log`；安装器未发布
半成品根，再次准备成功，见 `setup-node-retry.log`。这一宿主失败的确切
来源未定位，不能据此宣称根准备已无间歇性失败。

## 复现

```powershell
python tools/benchmark-dpkg-phases.py --root <测试根> --dist <发行目录> --output <新报告目录> --repeat 1 --files 512 --children 12 --apt --io-probe
python tools/test-common-workloads.py --root <开发工具根> --dist <发行目录> --output <新报告目录> --repeat 2 --only '^(gcc|clang|cpp-futures|cargo|cmake-ninja|git-workflow|archive-roundtrip|interpreter-command|dlopen-deepbind)$'
python -m unittest discover -s tools/guest-deps
```

已存在的根不会因重新打包自动迁移。离线接口修复作用于新生成的镜像；
含 libc 开发脚本的镜像在生成时完成脚本适配，后续上游软件包重新写入
该脚本的迁移不在这次打包变更中。
