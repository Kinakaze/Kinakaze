# 发布流程

`v*` 标签工作流在 Windows 检查通过后生成源码 ZIP、SHA-256 校验文件并正式发布。版本说明必须预先提交到 `docs/releases/<tag>.md`。若该版本已经发布，工作流只更新源码附件，保留版本说明与二进制附件。

## 准备版本

1. 在 `main` 完成目标修改，更新 `CHANGELOG.md` 中对应版本、日期与已知限制。
2. 更新根 `Cargo.toml` 的 `workspace.package.version`，同步并提交 `Cargo.lock`。首个正式版本为 `0.1.0`。
3. 按 [贡献指南](../CONTRIBUTING.md) 完成检查；影响客体行为的修改需提供实际行为验证。
4. 确认许可证、第三方声明、构建说明和链接仍与源码一致。

版本标签必须是 `v` 加上 workspace 版本。工作流拒绝标签与清单不一致的发布。

```powershell
# 替换为已提交到 main 的实际版本。
$Version = '0.1.0'
git tag -a "v$Version" -m "Kinakaze v$Version"
git push origin "v$Version"
```

标签应指向需要发布的确切提交。发布后修复通过新版本交付，不移动已有发行标签。

## 检查发行内容

推送标签前核对本地产物，发布后确认 `Source release` 成功并核对 Releases 附件：

- 源码包来自标签提交，包含构建源码、测试、许可证与第三方声明。
- SHA-256 与下载到本机的 ZIP 匹配。
- 解压后可以解析 workspace，并在所列环境中构建。
- 发布说明包含该版本变化、构建环境和已知限制。
- 正式版本不标记为草稿或预发布。兼容性仍以该版本测试范围为准。

没有推送标签时，普通提交只运行 CI，不创建 Release。

## 二进制发行

源码工作流不自动打包开发机的 `artifacts/` 或客体环境。提供 Windows 二进制前，还需：

1. 从独立构建目录运行 `./tools/build.ps1 -Release -TargetDirectory target/release-v0.1.0 -DistDirectory artifacts/release-v0.1.0`，保留对应提交和测试记录。
2. 运行 `tools/prepare-release-rootfs.py` 生成完整基础 rootfs 与首次安装清单，再运行 `tools/test-release-runtime.py` 验证入口、原生模块、首次安装及进程树。说明测试主机环境，不能把临时目录测试称为全新主机测试。
3. 随包附上 Kinakaze 许可证，以及实际包含的第三方代码、运行库和客体软件要求的声明。
4. 核对不包含缓存、日志、机器配置、账号数据、密钥、SDK 或未经确认可再分发的资源。
5. 按架构和版本命名归档，生成 SHA-256，并明确适用环境及未验证场景。

`tools/package-runtime.py --dist artifacts/release-v0.1.0 --report artifacts/release-runtime-report.json` 检查实际镜像哈希与安装清单后生成运行 ZIP。`tools/package-source.py --ref HEAD --tag v0.1.0` 生成相同提交的源码 ZIP。运行包会附带许可证、对应客体包源码和验证报告；源码缓存按 `config/release-sources.lock.json` 校验。

`-SkipTests` 的构建与历史构建的通过记录不能替代当前发行包的验证。
