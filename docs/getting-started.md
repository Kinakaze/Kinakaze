# 上手指南

## 环境

当前支持宿主 Windows x86-64 和客体 Linux x86-64 ELF。请安装：

1. Git、Python 3.11+。
2. Visual Studio Build Tools：C++ 桌面开发工具、x64 MSVC 编译工具和 Windows SDK。
3. Rust 的 `x86_64-pc-windows-msvc` 工具链。CI 使用 1.95.0；清单中的最低版本声明不代表每个最低版本组合都已验收。
4. LLVM，确保 `clang`、`lld-link` 与 `ld.lld` 在 `PATH` 中。

在配置好 x64 MSVC 环境的 PowerShell 中检查工具：

```powershell
rustc --version
cargo --version
python --version
Get-Command clang,lld-link,ld.lld,lib.exe
```

仓库的 `.cargo/config.toml` 使用 `tools/native-link.cmd` 合并链接输入。所有构建命令均在仓库根目录运行。

## 获取与构建

```powershell
git clone https://github.com/Kinakaze/Kinakaze.git
Set-Location Kinakaze
./tools/build.ps1 -DistDirectory artifacts/kinakaze-dist
```

默认构建包含格式、导出一致性检查、Rust/Python 测试和管理服务 smoke。仅需要快速检查源码时可先运行：

```powershell
cargo check --workspace --all-targets --locked
python -m unittest discover -s tools/native-exports
python -m unittest discover -s tools/guest-deps
```

优化构建使用 `./tools/build.ps1 -Release -DistDirectory artifacts/kinakaze-release`。开发调试可显式选择 `-SkipTests`，但这种结果不应标记为完整测试通过。导出变化需要 `-RefreshExports`；为客体 GCC/Clang 准备 ELF 链接输入时使用 `-Development`。

输出结构为：

```text
artifacts/kinakaze-dist/
  init.exe
  worker.exe
  rootfs/
    lib/                 # 原生兼容模块及运行库
    usr/share/doc/       # 随包第三方声明
```

客体程序和数据需要单独准备。原生模块仍依赖 Windows 系统和 VC Runtime；干净 Windows 安装上的独立部署尚待验收。

## 准备 Linux 文件树

`prepare-root.py` 要求已有 Linux x86-64 输入文件树，至少包含 `usr/bin/busybox`、`usr/bin/curl` 和所需 ELF 依赖。按需提供 `/etc` 配置、CA 证书、应用程序及资源文件。输入根目录不能与输出根目录重叠。

```powershell
$GuestSource = 'C:/path/to/linux-root' # 替换为实际输入目录
python tools/prepare-root.py --source "$GuestSource" --root artifacts/guest-root --dist artifacts/kinakaze-dist --check-only
python tools/prepare-root.py --source "$GuestSource" --root artifacts/guest-root --dist artifacts/kinakaze-dist
```

`--dist` 必须指向刚才生成的原生包，用于判断哪些依赖由 Kinakaze 提供。`--check-only` 验证依赖而不复制客体输出；它仍可能下载并缓存锁文件中的软件包。已有完整缓存时追加 `--offline`。

`--program` 添加程序，`--tree` 添加资源目录，`--package` 添加锁定的软件包，`--java` 与 `--java-home` 配置已有 Linux JRE。详见 [依赖工具文档](../tools/guest-deps/README.md)。

## 运行

```powershell
./artifacts/kinakaze-dist/worker.exe run --root artifacts/guest-root -- /usr/bin/busybox echo hello
./artifacts/kinakaze-dist/worker.exe run --root artifacts/guest-root -- /bin/sh
```

worker 默认从自身所在目录寻找原生模块；`--root` 是客体文件树，`--` 后是 Linux 程序路径与参数。客体程序路径采用绝对 Linux 路径。若程序和依赖已放入发布目录相邻的 `rootfs/`，可省略 `--root`。

可选只读进程页面：

```powershell
./artifacts/kinakaze-dist/worker.exe run --root artifacts/guest-root --web 127.0.0.1:0 -- /bin/sh
```

地址由程序输出。页面仅在显式启用时运行，随管理会话结束关闭。

## 常见问题

| 现象 | 检查方向 |
| --- | --- |
| 找不到链接器、MSVC 库或 Windows SDK | 使用配置好 MSVC 的 PowerShell，检查 LLVM 与 `lib.exe` |
| `Stale generated exports` | 使用同一源码和构建配置；确需变更时执行 `-RefreshExports` 并审查生成差异 |
| `duplicate native image` | 构建目录可能混有改名前或其他配置的 DLL；确认没有进程占用后用 `cargo clean` 清理可重建的 `target/`，再重新构建 |
| 找不到 BusyBox/curl 或未知 ELF 依赖 | 检查输入布局、程序架构、依赖锁和 `--dist`；缓存不能代替缺失的任意输入 |
| 符号存在但应用运行失败 | 用最小行为探针复现，记录提交号和客体版本；导出存在不等于 ABI 行为完整 |

应用专项命令见 [文档导航](README.md)。
