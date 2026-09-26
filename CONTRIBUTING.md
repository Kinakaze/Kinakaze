# 贡献指南

开发环境与首次构建请参考 [上手指南](docs/getting-started.md)。

## 问题与改动

提交 Issue 前请搜索已有问题。错误报告应包含源码提交号、Windows/Rust/Python 版本、实际命令、预期结果与最小复现。涉及 Linux 应用时，还需提供程序版本、ELF 架构与依赖来源。日志中请移除令牌、密码、私钥及个人数据。

较大的架构或 ABI 调整请先用 Issue 说明需求。Pull Request 应围绕一个问题，描述行为变化和验证结果；未运行或未通过的测试应明确列出。

## 本地检查

从仓库根目录执行：

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
python -m unittest discover -s tools/native-exports
python -m unittest discover -s tools/guest-deps
```

修改原生导出、模块依赖或打包路径后，再执行：

```powershell
./tools/build.ps1 -DistDirectory artifacts/kinakaze-dist
```

`exports.def` 和 `src/object_layout.rs` 中的生成内容由 `tools/native-exports/generate.py` 维护。需要更新导出时使用 `./tools/build.ps1 -RefreshExports`，将生成结果与对应实现一起提交。

CI 覆盖格式、全目标编译检查、核心单元测试、Python 工具测试、原生构建与导出一致性。图形、音频、GPU、客体系统和完整应用验收需要对应环境；CI 通过不能替代这些验证。

## 源码与依赖

- 为行为变化增加有针对性的回归验证；文档和纯格式改动无需镜像实现的测试。
- 保持宿主与客体 ABI 边界明确，注明跨模块内存和句柄的所有权。
- 新增客体下载输入时更新依赖锁与来源证据，保留上游版权文件。
- 保留第三方源码的版权和许可声明，说明复制或改编代码的来源。
- 构建产物、客体文件树、日志、下载缓存和本机配置均应留在忽略目录。

除明确标注的第三方内容外，项目贡献适用仓库的 MIT OR Apache-2.0 许可。讨论请围绕可复现的问题和代码本身，并尊重其他参与者。
