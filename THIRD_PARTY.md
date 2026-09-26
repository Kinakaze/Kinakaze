# 第三方内容与依赖

根目录的 MIT OR Apache-2.0 声明适用于 Kinakaze 自有代码。以下内容保留其原有版权与许可证。

## 仓库内的第三方内容

| 内容 | 来源与许可说明 |
| --- | --- |
| [`libs/libm/src/ld80/powl.c`](libs/libm/src/ld80/powl.c) | 来自 musl v1.2.5 / OpenBSD / Stephen L. Moshier；文件开头保留版权和许可全文，背景见 [目录说明](libs/libm/src/ld80/README.md) |
| [`libs/libc/third-party-notices.txt`](libs/libc/third-party-notices.txt) | `rustc_apfloat` 及相关 LLVM 来源声明；打包器会复制到客体 `usr/share/doc/kinakaze-libc/copyright` |

## Rust 依赖

依赖版本和校验和由 [`Cargo.lock`](Cargo.lock) 固定。各依赖以其自身声明为准；`cargo metadata --locked --format-version 1` 可列出依赖及许可元数据。发布包含依赖代码的二进制时，应随包提供实际所用依赖要求的许可文本和通知。

## Linux 客体、工具链与应用

[`tools/guest-deps/dependencies.lock.json`](tools/guest-deps/dependencies.lock.json) 记录选定软件包的来源、哈希和版权材料，处理方式见 [客体依赖说明](tools/guest-deps/README.md)。下载缓存、客体文件树、JRE、游戏资源和 SDK 均不属于源码发布包。

MSVC/Windows SDK、LLVM、Rust 运行库及下载的第三方程序各自适用其许可。再分发二进制或预装客体环境前，应核对实际包含的文件及对应许可；源码仓库的许可证不替代这些许可。
