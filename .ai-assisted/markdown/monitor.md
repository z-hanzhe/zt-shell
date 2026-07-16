远程监控

采集 collect通过SessionManager::exec执行MONITOR_SCRIPT一次性远程命令 脚本###段名###分隔区块 CPU与网卡各采样两次(间隔0.5秒)算速率 无需服务端状态

数据源 /etc/os-release /proc/uptime /proc/cpuinfo /proc/loadavg /proc/stat(两次差值算CPU) /proc/meminfo /proc/net/dev(跳过lo) df -kP(跳过tmpfs devtmpfs overlay) ps -eo pid,comm,%cpu,%mem,rss(CPU降序 rss为KiB转字节) 物理网卡靠/sys/class/net/*/device存在判定

解析 split_sections切分 parse_stat/parse_net/mem_field各取所需 仅Linux MonitorData字段见types.ts

前端 store按会话维度采集 会话建立start关闭stop 与激活选项卡无关 间隔取settings
