# 远程监控

## 职责与入口

- `ssh/monitor.rs` 的 `collect` 通过现有 SSH 会话执行一次性远端命令并返回监控数据。
- `stores/monitor.ts` 按会话持续采集和缓存，`MonitorPanel.vue` 仅负责展示与网卡选择。
- 采集周期由 `stores/settings.ts` 的应用设置控制。

## 数据与限制

- 数据来自 Linux 的 `/proc`、`/etc/os-release`、`df`、`ps` 和网卡信息；包括系统信息、CPU、内存、磁盘、进程与网卡速率。
- 监控随会话建立启动、随会话关闭停止，与当前激活的选项卡无关。
- ⚠️ 陷阱：监控脚本依赖 Linux 数据源和常用命令；扩展到其他远端系统前，必须新增对应采集实现与解析逻辑。
