# 更新记录

## 0.1.0 — 2026-09-26

- 增加可选 rootfs 清单，外部传入优先，仅初始化不存在或空的 rootfs；完整预检、源文件 SHA-256、并发锁与暂存目录提交避免留下半成品。
- 增加长驻 init 的会话文件和 `init launch --parent PID`，支持新客户端向已有进程树启动新进程、设置 cwd/环境并等待退出。
- 启动凭据与 worker 凭据分离，会话文件限制为当前用户，客户端验证宿主 PID、创建时间和管道对端。
- 基础发行 rootfs 与初始化资源从已锁定的 BusyBox/netbase 包生成，不依赖旧工程目录；附带源码、版权材料和 SHA-256。
- Java/Minecraft 工具根据已安装版本发现路径，多个候选时要求显式选择；构建目录可配置，避免旧 DLL 污染 release。
- 修复全工作区 Clippy 检查阻断项，包括公开裸指针接口的安全约定、内部接口可见性和无效比较，清理未使用常量与导入。

- 项目统一命名为 Kinakaze，源码发布在 `Kinakaze/Kinakaze`。
- 整理首页、上手文档、贡献说明、安全报告渠道与发布流程。
- 补齐 MIT / Apache-2.0 许可证正文及第三方声明导航。
- 增加 Windows CI 与标签发布工作流。

使用与验证见 [v0.1.0 运行说明](docs/runtime-release-v0.1.0.md)。完整 Linux ABI、容器隔离和干净 Windows 环境的独立部署仍未完成验收；历史应用结果见 [开发进度](docs/progress.md) 和 [验证记录](docs/validation.md)。工作区仍有风格与文档类警告，本版本不宣称零警告。
