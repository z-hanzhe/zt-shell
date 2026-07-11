远程监控

采集 见src-tauri/src/ssh/monitor.rs collect通过SessionManager::exec执行MONITOR_SCRIPT一次性远程命令 脚本用标记###段名###分隔各区块 对CPU与网卡各采样两次间隔0.5秒以计算速率 无需服务端状态

数据源 hostname主机名 os取/etc/os-release的PRETTY_NAME kernel用uname -r uptime读/proc/uptime cpu核数grep /proc/cpuinfo loadavg读/proc/loadavg cpu使用率两次/proc/stat求差 内存读/proc/meminfo用MemTotal与MemAvailable算已用 交换区SwapTotal SwapFree 网卡两次/proc/net/dev求速率跳过lo 磁盘df -kP跳过tmpfs devtmpfs overlay 进程ps按cpu降序取前列

解析 split_sections切分区块 parse_stat取总时间与空闲时间 parse_net取各网卡收发字节 mem_field取meminfo字段kB转字节

前端 见MonitorPanel.vue 定时monitorCollect采集 默认3秒 分模块展示系统信息CPU内存网卡磁盘进程 进度条颜色按占用率90红70黄其余蓝 会话切换或断开时启停定时器

数据结构 MonitorData含hostname os kernel uptime cpuCount cpuUsage loadAvg memTotal memUsed memAvailable swapTotal swapUsed netInterfaces disks processes 前后端类型对应见src/types.ts

限制 仅支持Linux远端 依赖标准/proc与coreutils
