# 文档导航

## 使用与参与

- [上手指南](getting-started.md)：构建环境、客体准备、运行与常见问题。
- [贡献指南](../CONTRIBUTING.md)：问题报告、开发检查与 Pull Request。
- [发布流程](releasing.md)：版本、源码发行包与二进制验收。
- [安全问题](../SECURITY.md) · [第三方声明](../THIRD_PARTY.md) · [更新记录](../CHANGELOG.md)。

## 架构与开发

- [整体设计](architecture-v2.md) 与 [syscall 清单](architecture-v2-syscalls.csv)。
- [ABI 路线](abi-roadmap.md)、[实现约定](implementation-contract.md) 和 [目标](goal.md)。
- [原生桥接](../crates/bridge/README.md)、[进程管理](../crates/manager/README.md)、[执行引擎](../engine/crates/guest-engine/README.md)。
- [客体依赖准备](../tools/guest-deps/README.md) 与 [CUDA provider](../libs/libcuda/README.md)。

## 兼容性与性能

- [集成验证记录](validation.md) 与 [开发进度](progress.md)。
- [软件运行边界](software-boundaries-2026-09-22.md) 与 [浏览器启动](browser-startup-2026-09-22.md)。
- [Minecraft 渲染](minecraft-rendering-2026-09-23.md)、[GNOME](gnome.md) 与 [网易云音乐](netease-cloud-music-2026-09-22.md)。
- [启动性能基线](startup-performance-2026-09-22.md)、[第二轮](startup-performance-round2-2026-09-22.md)、[第三轮](startup-performance-round3-2026-09-22.md)、[第四轮](startup-performance-round4-2026-09-23.md)、[第五轮](startup-performance-round5-2026-09-23.md)、[第六轮](startup-performance-round6-2026-09-23.md)。
- [跨进程共享 futex](futex-shared-2026-09-23.md)。

带日期的记录描述当时的源码、产物与测试环境，其中的本机路径和历史命令用于追溯，可能不再适用于当前版本。首次构建请以 [上手指南](getting-started.md) 为入口；忽略目录 `artifacts/` 中的原始日志需要本地重新生成。
