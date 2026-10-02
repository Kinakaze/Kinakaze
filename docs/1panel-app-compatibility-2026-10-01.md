# 1Panel 商店应用兼容性实测（2026-10-01）

测试对象是商店中的应用程序，未启动 1Panel。商店清单固定在
`92b569c887b23b50ad7dbc4c013c82514932a47b`，共 265 项。
测试使用 Debian 包或固定版本的上游 Linux 程序；这不等同于商店 Docker
镜像、全部版本或全部功能兼容。当前尚未完成全部应用兼容。

## 清单与证据

`tools/test-1panel-apps.py --store` 从 Git 提交树枚举应用目录，并对提交中的
`data.yml` 计算 SHA-256。稀疏检出、工作区修改以及 `--only` 均不会缩小
覆盖率分母。未注册的应用记为 `missing_scenario`，未安装的程序记为
`missing_application`。重复测试必须每轮通过才计入通过数。

报告保存发行目录二进制哈希、Debian 包状态、测试目录及逐项结果。
最新执行的场景还保存测试脚本哈希。不同发行目录的结果不能直接相加，
单项后续通过也不会改写此前全量运行中的失败记录。

主要结果文件：

- [全量运行，dist-c11](../artifacts/1panel-apps-continued/full-store-c11/results.json)
- [全量运行，dist-create](../artifacts/1panel-apps-continued/full-store-final/results.json)
- [AstrBot 预装 WebUI 后的复测](../artifacts/1panel-apps-continued/astrbot-bundled/results.json)
- [Elasticsearch 恢复等待修正后的复测](../artifacts/1panel-apps-continued/elasticsearch-health-fixed/results.json)
- [Elasticsearch、Nextcloud 测试准备调整后的复测](../artifacts/1panel-apps-continued/setup-fixed/results.json)
- [RabbitMQ 曾通过的定向运行](../artifacts/1panel-apps-continued/rabbitmq-direct/results.json)
- [程序下载地址、版本与 SHA-256](../artifacts/1panel-apps-continued/cache/artifacts.json)

`dist-create` 全量运行的 265 项结果：

| 状态 | 数量 |
| --- | ---: |
| passed | 27 |
| failed | 10 |
| timeout | 1 |
| missing_application | 8 |
| missing_scenario | 219 |

随后在同一发行目录对 Elasticsearch、Nextcloud 分别复测通过，耗时 83.538 秒、
57.076 秒。此前全量报告中的失败、超时记录保留。AstrBot、Gitea 在本次全量
运行中通过；Gitea 较早的定向测试仍有 Go 崩溃记录，稳定性需要继续排查。

## 已实现的修复

- 文件映射：Windows section view 使用实际文件字节长度，保留独立的页对齐
  地址空间长度；覆盖部分尾页、文件增长和 `PROT_NONE` 后恢复访问。
- Erlang：补齐 `_SC_IOV_MAX` 和 `closefrom`，保持边界及 errno 语义。
- Python 3.12：补齐历史 pthread 转发及 `__isnanl` 等导出；长双精度判断
  使用真实 System V 80 位参数 ABI。
- ClickHouse：实现其依赖使用的 C11 线程、once、互斥量和条件变量接口。
- 原生文件创建：含 `.`、`..` 的路径通过目录句柄解析后，直接返回创建时
  获得的访问句柄。新文件设为 `0444` 不再错误地拒绝最初的写入句柄；
  后续写入打开仍拒绝，父目录创建权限仍检查。该问题曾阻止 Gitea 写入 Git 对象。

## 应用场景修正

- AstrBot 固定为 v4.28.2，源提交
  `3c7adafa1397e182d60b1016bf88759265113c8a`，使用 Python 3.12.11。
  依赖通过兼容层内的 pip 安装，并保存 pip 安装报告。
- WebUI 使用同版本官方发布包，SHA-256 为
  `20a85309adf0802c66c70140a729d11890665521aa928310d150f74c3464d66f`。
  使用 `--webui-dir` 指定预装资源，避免每次隔离测试启动时联网下载。
  场景检查实际页面、未授权拒绝、登录、配置创建读取以及重启后持久化。
  SIGTERM 按该应用的默认终止行为验证，不宣称优雅退出；未验证外部消息平台或付费模型调用。
- ClickHouse 的修改 SQL 使用 HTTP POST，避免 GET 的只读限制。
- Elasticsearch 复制官方日志配置，并等待分片恢复到 yellow/green。
  恢复期等待请求返回的 HTTP 408 仅表示尚未就绪，继续在总超时内等待。
  写入客户端等待 60 秒，覆盖服务端默认等待时间；调整后完整场景通过。
- Nextcloud 使用官方 `NEXTCLOUD_CONFIG_DIR` 隔离配置，数据目录仍独立，
  避免在每次场景启动前通过兼容层复制整套源码。调整后安装、SQLite、
  WebDAV 鉴权上传下载及重启持久化场景通过。
- RabbitMQ 直接启动上游脚本，使用独立合法范围的分布式端口，避免 Debian
  `su` 包装层的退出转发问题。验证持久化队列、消息发布和重启后消费。

## 回归与未解决项

主机统计回归 4 项、native exports 单元测试 25 项通过。
`dist-create` 下 C11、运行时限制及文件映射回归各通过两轮。
只读文件创建用例修复前在 `/./dot` 返回 EACCES，修复后通过两轮：
[修复前](../artifacts/1panel-apps-continued/readonly-before/results.json)、
[修复后](../artifacts/1panel-apps-continued/readonly-fixed/results.json)。

全量结果仍包含大量未覆盖应用及实际失败，应以结果 JSON 的状态为准。
已观察到的问题包括：

- Grafana 和部分 Gitea 运行出现 Go 崩溃或栈回溯错误。Grafana 关闭异步抢占
  的诊断对照曾通过，但没有把关闭抢占作为正式修复。
- RabbitMQ 有通过记录，也有启动超时，尚不能宣称稳定通过。
- MongoDB 写入后重启时 WiredTiger 报十六进制格式错误。
- Meilisearch、WordPress 存在就绪或请求超时；Halo 返回 HTTP 500。
- Alist/OpenList 启动崩溃；Node Exporter 的若干 `/proc` 数据源尚未实现。
- 目录符号链接在 Python 临时目录清理中曾被 unlink 错判为目录；当前文件
  创建回归只覆盖普通路径及 `.`、`..`，不声称覆盖该链接清理问题。

[扩展 ABI 扫描](../artifacts/1panel-apps-continued/abi-final.json) 检查了 Python 3.12
安装目录、Erlang 及 librdkafka，共 220 个 ELF、1705 项导入要求；仍有 14 项
符号或版本要求未满足，包括多个长双精度数学函数，以及 `vsyslog`、`fabsf`、
`truncl`、`pthread_attr_setaffinity_np` 的版本导出。不能将已通过的启动场景
解释为这些目录中所有扩展模块都兼容。

## 重跑

```powershell
python tools/test_1panel_reporting.py
python -m unittest discover -s tools/native-exports -p test_exports.py
python tools/test-server-features.py --root artifacts/1panel-apps-20261001/root --dist artifacts/1panel-apps-continued/dist-create --output artifacts/1panel-apps-continued/recheck --only readonly-create c11-threads runtime-limits mapped-file-growth --repeat 2
python tools/test-1panel-apps.py --root artifacts/1panel-apps-20261001/root --dist artifacts/1panel-apps-continued/dist-create --store artifacts/1panel-appstore-upstream --output artifacts/1panel-apps-continued/recheck-store --timeout 240
```

准备新的 AstrBot 测试根时，下载 `astrbot`、`python312` 和 `astrbot-dashboard`
三项，再运行 `tools/provision-1panel-apps.py --astrbot` 安装依赖。
`--offline` 只校验和重用下载锁中的归档，不代替 Python 依赖安装。
