//! 远程系统监控数据采集与解析
//!
//! 通过一条自包含的远程命令读取 /proc 与 df/ps 输出（对 CPU 与网卡各采样两次），
//! 在 Rust 侧解析为结构化监控数据，避免维护服务端状态

use anyhow::Result;
use serde::Serialize;

use super::manager::SessionManager;

/// 采集监控数据的远程命令。两次采样间隔 0.5 秒用于计算 CPU 与网卡速率
const MONITOR_SCRIPT: &str = r#"
echo '###HOST###'; hostname 2>/dev/null
echo '###OS###'; (grep PRETTY_NAME /etc/os-release 2>/dev/null | cut -d= -f2 | tr -d '"')
echo '###KERNEL###'; uname -r 2>/dev/null
echo '###UPTIME###'; cat /proc/uptime 2>/dev/null
echo '###CPUCOUNT###'; grep -c ^processor /proc/cpuinfo 2>/dev/null
echo '###LOADAVG###'; cat /proc/loadavg 2>/dev/null
echo '###STAT1###'; head -1 /proc/stat 2>/dev/null
echo '###MEM###'; cat /proc/meminfo 2>/dev/null
echo '###NET1###'; cat /proc/net/dev 2>/dev/null
echo '###DISK###'; df -kP 2>/dev/null
echo '###PROC###'; ps -eo pid,comm,%cpu,%mem --sort=-%cpu 2>/dev/null | head -16
sleep 0.5
echo '###STAT2###'; head -1 /proc/stat 2>/dev/null
echo '###NET2###'; cat /proc/net/dev 2>/dev/null
echo '###END###'
"#;

/// 网卡监控数据
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetInterface {
    /// 网卡名称
    pub name: String,
    /// 接收速率（字节/秒）
    pub rx_rate: u64,
    /// 发送速率（字节/秒）
    pub tx_rate: u64,
    /// 累计接收字节数
    pub rx_total: u64,
    /// 累计发送字节数
    pub tx_total: u64,
}

/// 磁盘分区使用情况
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    /// 文件系统设备
    pub filesystem: String,
    /// 挂载点
    pub mount: String,
    /// 总容量（字节）
    pub total: u64,
    /// 已用（字节）
    pub used: u64,
    /// 可用（字节）
    pub available: u64,
    /// 使用率百分比
    pub use_percent: f64,
}

/// 进程占用信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    /// 进程 ID
    pub pid: u32,
    /// 进程名
    pub name: String,
    /// CPU 占用百分比
    pub cpu: f64,
    /// 内存占用百分比
    pub mem: f64,
}

/// 一次采集得到的完整监控数据
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MonitorData {
    /// 主机名
    pub hostname: String,
    /// 操作系统描述
    pub os: String,
    /// 内核版本
    pub kernel: String,
    /// 运行时长（秒）
    pub uptime: u64,
    /// 逻辑 CPU 核心数
    pub cpu_count: u32,
    /// CPU 总体使用率（0-100）
    pub cpu_usage: f64,
    /// 系统负载（1、5、15 分钟）
    pub load_avg: [f64; 3],
    /// 内存总量（字节）
    pub mem_total: u64,
    /// 内存已用（字节）
    pub mem_used: u64,
    /// 内存可用（字节）
    pub mem_available: u64,
    /// 交换区总量（字节）
    pub swap_total: u64,
    /// 交换区已用（字节）
    pub swap_used: u64,
    /// 网卡列表
    pub net_interfaces: Vec<NetInterface>,
    /// 磁盘分区列表
    pub disks: Vec<DiskUsage>,
    /// 进程列表（按 CPU 降序）
    pub processes: Vec<ProcessInfo>,
}

/// 将原始输出按 `###标记###` 切分为各区块
fn split_sections(raw: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let mut buffer = String::new();
    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("###") && trimmed.ends_with("###") && trimmed.len() > 6 {
            if let Some(key) = current.take() {
                map.insert(key, buffer.trim().to_string());
            }
            buffer.clear();
            let key = trimmed.trim_matches('#').to_string();
            current = Some(key);
        } else if current.is_some() {
            buffer.push_str(line);
            buffer.push('\n');
        }
    }
    if let Some(key) = current.take() {
        map.insert(key, buffer.trim().to_string());
    }
    map
}

/// 解析 /proc/stat 首行，返回 (总时间, 空闲时间)
fn parse_stat(line: &str) -> (u64, u64) {
    let nums: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|n| n.parse().ok())
        .collect();
    if nums.len() < 4 {
        return (0, 0);
    }
    let total: u64 = nums.iter().sum();
    // idle = idle + iowait
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    (total, idle)
}

/// 解析 /proc/net/dev，返回各网卡 (rx_bytes, tx_bytes)
fn parse_net(raw: &str) -> std::collections::HashMap<String, (u64, u64)> {
    let mut map = std::collections::HashMap::new();
    for line in raw.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        if name.is_empty() || name == "lo" {
            continue;
        }
        let fields: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        if fields.len() >= 9 {
            map.insert(name, (fields[0], fields[8]));
        }
    }
    map
}

/// 从 meminfo 中提取指定字段的 kB 值
fn mem_field(raw: &str, key: &str) -> u64 {
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start_matches(':').trim();
            if let Some(num) = rest.split_whitespace().next() {
                return num.parse::<u64>().unwrap_or(0) * 1024;
            }
        }
    }
    0
}

/// 采集并解析一次监控数据
pub async fn collect(manager: &SessionManager, session_id: &str) -> Result<MonitorData> {
    let raw = manager.exec(session_id, MONITOR_SCRIPT).await?;
    let sections = split_sections(&raw);
    let get = |k: &str| sections.get(k).cloned().unwrap_or_default();

    let mut data = MonitorData {
        hostname: get("HOST"),
        os: get("OS"),
        kernel: get("KERNEL"),
        cpu_count: get("CPUCOUNT").trim().parse().unwrap_or(0),
        ..Default::default()
    };

    // 运行时长
    if let Some(first) = get("UPTIME").split_whitespace().next() {
        data.uptime = first.parse::<f64>().unwrap_or(0.0) as u64;
    }

    // 负载
    let load: Vec<f64> = get("LOADAVG")
        .split_whitespace()
        .take(3)
        .filter_map(|n| n.parse().ok())
        .collect();
    for (i, v) in load.into_iter().enumerate().take(3) {
        data.load_avg[i] = v;
    }

    // CPU 使用率（两次采样求差）
    let (total1, idle1) = parse_stat(&get("STAT1"));
    let (total2, idle2) = parse_stat(&get("STAT2"));
    let total_delta = total2.saturating_sub(total1);
    let idle_delta = idle2.saturating_sub(idle1);
    if total_delta > 0 {
        data.cpu_usage =
            ((total_delta - idle_delta) as f64 / total_delta as f64 * 100.0).clamp(0.0, 100.0);
    }

    // 内存
    let mem = get("MEM");
    data.mem_total = mem_field(&mem, "MemTotal");
    data.mem_available = mem_field(&mem, "MemAvailable");
    data.mem_used = data.mem_total.saturating_sub(data.mem_available);
    data.swap_total = mem_field(&mem, "SwapTotal");
    let swap_free = mem_field(&mem, "SwapFree");
    data.swap_used = data.swap_total.saturating_sub(swap_free);

    // 网卡速率（0.5 秒采样间隔）
    let net1 = parse_net(&get("NET1"));
    let net2 = parse_net(&get("NET2"));
    let interval = 0.5_f64;
    for (name, (rx2, tx2)) in &net2 {
        let (rx1, tx1) = net1.get(name).copied().unwrap_or((*rx2, *tx2));
        data.net_interfaces.push(NetInterface {
            name: name.clone(),
            rx_rate: ((rx2.saturating_sub(rx1)) as f64 / interval) as u64,
            tx_rate: ((tx2.saturating_sub(tx1)) as f64 / interval) as u64,
            rx_total: *rx2,
            tx_total: *tx2,
        });
    }
    data.net_interfaces.sort_by(|a, b| a.name.cmp(&b.name));

    // 磁盘
    for line in get("DISK").lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        // 跳过虚拟文件系统
        if f[0].starts_with("tmpfs") || f[0].starts_with("devtmpfs") || f[0] == "overlay" {
            continue;
        }
        let total = f[1].parse::<u64>().unwrap_or(0) * 1024;
        let used = f[2].parse::<u64>().unwrap_or(0) * 1024;
        let available = f[3].parse::<u64>().unwrap_or(0) * 1024;
        let use_percent = f[4].trim_end_matches('%').parse::<f64>().unwrap_or(0.0);
        data.disks.push(DiskUsage {
            filesystem: f[0].to_string(),
            mount: f[5..].join(" "),
            total,
            used,
            available,
            use_percent,
        });
    }

    // 进程（首行为表头）
    for line in get("PROC").lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        data.processes.push(ProcessInfo {
            pid: f[0].parse().unwrap_or(0),
            name: f[1].to_string(),
            cpu: f[2].parse().unwrap_or(0.0),
            mem: f[3].parse().unwrap_or(0.0),
        });
    }

    Ok(data)
}
