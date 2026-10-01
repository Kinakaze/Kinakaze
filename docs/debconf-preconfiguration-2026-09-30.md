# debconf 预配置错误修复

## 根因

基础镜像用 `kinakaze-base` 的版本化 `Provides` 记录已经适配的 debconf 组件，
没有声称独立安装、配置过 Debian 的 debconf 包。APT 2.6.1 的
[`apt-extracttemplates`](https://github.com/Debian/apt/blob/2.6.1/cmdline/apt-extracttemplates.cc#L259)
只读取名为 `debconf` 的包的 `CurrentVer()`，不读取版本化的 provider，因而先报：

```text
E: Cannot get debconf version. Is debconf installed?
```

随后 [debconf 1.5.82 的 dpkg-preconfigure](https://sources.debian.org/src/debconf/1.5.82/dpkg-preconfigure/)
在子命令非零退出时打印 Perl 的 `$!`，此处残留 ENOTTY，形成：

```text
debconf: apt-extracttemplates failed: Inappropriate ioctl for device
```

这次复现中的直接原因是组件版本查询失败。

## 修改

- `config/rootfs.manifest.json` 直接声明 `/usr/bin/apt-extracttemplates` 的适配脚本，
  并声明该脚本及原始提取器的 `0755` 权限。脚本为这一个读取端生成临时状态视图，版本来自当前
  已安装 `kinakaze-base` 的 `Provides: debconf (= ...)`。真实 dpkg 状态文件保持原样。
- Debian 原版 ELF 保留在 `/usr/lib/kinakaze/apt-extracttemplates`，继续负责归档解析、
  依赖版本比较、模板和配置脚本提取及错误报告。临时状态视图在子命令退出后清理。
- 独立的 debconf 记录优先，包括未完成配置的状态。缺少真实版本化 provider 时不生成版本。
- 保留 `-o`、`-c` 配置选择、带空格的路径、`--`、`RootDir` 和原命令返回码。
- 发行清单生成器在应用 manifest 覆盖前保留原始 ELF，再将两者一起纳入基础包的
  文件所有权与校验记录。在线清单从锁定的 `apt-utils` 归档读取原始 ELF；离线清单使用带 SHA-256 的种子文件。

该修改接入新基础镜像的生成流程。已创建的 rootfs 不会被构建脚本自动改写。
本次独立验证使用修复后的新 rootfs：
`artifacts/node-install-20260930/p180-debconf-root`。
对应离线清单为 `artifacts/node-install-20260930/seed/rootfs-debconf.manifest.json`；
原有测速清单保留，便于复查修复前的结果。

完整发行流程验证使用 `prepare-release-rootfs.py --offline --online`，从本地已校验的
306 个锁定包生成含 16,135 个文件的在线发行清单：
`artifacts/install-180-20260930/debconf-manifest-final-dist/rootfs.manifest.json`。
该清单成功创建全新 `artifacts/node-install-20260930/p180-debconf-manifest-root`，
真实模板提取、预配置和 `apt-get install` 再次通过；成功命令的 stderr 均为空。

## 验证

5 项 bootstrap 单元测试和 2 项 release rootfs 清单回归通过。
清单回归覆盖在线、离线两种模式，检查适配器内容、原始 ELF、归档来源、可执行权限及基础包文件归属和校验值。
`tools/test-debconf-preconfigure.py` 构造真实 Debian 归档，验证：

1. 修复前稳定复现 `Cannot get debconf version`，未能提取模板。
2. 修复后提取模板及配置脚本成功；高于实际组件版本的依赖仍被原提取器正确排除。
3. 命令行指定另一份状态文件中的 debconf 1.5.99 时，正确使用该版本。
4. 手动预配置及 `dpkg-preconfigure --apt` 管道调用均成功，配置脚本确实写入
   `kinakaze-debconf-probe/answer=accepted`，通过真实 debconf 协议读回。
5. 实际 `apt-get install` 安装测试包成功，stderr 为空；随后正常清除测试包。
6. 损坏的 `.deb` 仍返回 100，并报告归档错误。
7. 提取与预配置前后的真实 dpkg status 字节完全一致；实际安装测试包后也没有新增独立 debconf 记录。

结果及完整命令输出：

- `artifacts/install-180-20260930/debconf-before/report.json`
- `artifacts/install-180-20260930/debconf-apt-final/report.json`
- `artifacts/install-180-20260930/debconf-manifest-report.json`
- `artifacts/install-180-20260930/debconf-manifest-apt/report.json`

这项修复没有重新运行完整的 357 包 Node 安装；上述实际安装使用专门的 debconf 测试包。
