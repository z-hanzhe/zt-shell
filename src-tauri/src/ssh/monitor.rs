//! 远程系统监控数据采集与解析
//!
//! 通过一条自包含的远程命令读取 /proc 与 df/ps 输出（对 CPU 与网卡各采样两次），
//! 在 Rust 侧解析为结构化监控数据，避免维护服务端状态

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Serialize;
use tokio::time::timeout;

use super::manager::SessionManager;

/// 单次监控采集允许的最长时间
const MONITOR_TIMEOUT: Duration = Duration::from_secs(15);

/// 采集监控数据的远程命令。两次采样间隔 0.5 秒用于计算 CPU 与网卡速率
const MONITOR_SCRIPT: &str = r#"
LC_ALL=C; export LC_ALL
echo '###HOST###'; hostname 2>/dev/null
echo '###OS###'; (grep PRETTY_NAME /etc/os-release 2>/dev/null | cut -d= -f2 | tr -d '"')
echo '###KERNELNAME###'; uname -s 2>/dev/null
echo '###KERNEL###'; uname -r 2>/dev/null
echo '###ARCH###'; uname -m 2>/dev/null
echo '###UPTIME###'; cat /proc/uptime 2>/dev/null
echo '###CPUCOUNT###'; (getconf _NPROCESSORS_ONLN 2>/dev/null || grep -c ^processor /proc/cpuinfo 2>/dev/null)
echo '###CPUINFO###'; awk -F: '
{
  key=tolower($1); gsub(/^[ \t]+|[ \t]+$/, "", key)
  value=$2
  if (key == "processor" && value !~ /[[:alpha:]]/) next
  if ((key == "model name" || key == "hardware" || key == "model" || key == "processor" ||
       key == "cpu mhz" || key == "clock" || key == "cache size" || key == "bogomips") && !seen[key]++) print
}' /proc/cpuinfo 2>/dev/null
echo '###LOADAVG###'; cat /proc/loadavg 2>/dev/null
echo '###MEM###'; cat /proc/meminfo 2>/dev/null
echo '###PHYS###'; for i in /sys/class/net/*; do [ -e "$i/device" ] && basename "$i"; done 2>/dev/null
echo '###DISK###'; df -kP 2>/dev/null
echo '###PROC###'; ps -eo pid,comm,%cpu,%mem,rss --sort=-%cpu 2>/dev/null | head -16
echo '###STAT1###'; head -1 /proc/stat 2>/dev/null
echo '###NET1###'; cat /proc/net/dev 2>/dev/null
echo '###NETTIME1###'; cat /proc/uptime 2>/dev/null
sleep 0.5
echo '###STAT2###'; head -1 /proc/stat 2>/dev/null
echo '###NET2###'; cat /proc/net/dev 2>/dev/null
echo '###NETTIME2###'; cat /proc/uptime 2>/dev/null
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
    /// 是否为物理网卡（依据 /sys/class/net/<name>/device 是否存在判定）
    pub is_physical: bool,
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
    /// 实际内存占用（字节，来自 RSS）
    pub mem_bytes: u64,
}

/// CPU 各类别占用率
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CpuUsageBreakdown {
    /// 用户态占用率
    pub user: f64,
    /// 系统态占用率
    pub system: f64,
    /// 调整过优先级的用户态占用率
    pub nice: f64,
    /// 空闲占比
    pub idle: f64,
    /// I/O 等待占比
    pub io_wait: f64,
    /// 硬件中断占用率
    pub irq: f64,
    /// 软件中断占用率
    pub soft_irq: f64,
    /// 虚拟机被宿主机占用的时间占比
    pub steal: f64,
}

/// 一次采集得到的完整监控数据
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MonitorData {
    /// 主机名
    pub hostname: String,
    /// 操作系统描述
    pub os: String,
    /// 内核名称
    pub kernel_name: String,
    /// 内核版本号
    pub kernel: String,
    /// 硬件架构
    pub architecture: String,
    /// 运行时长（秒）
    pub uptime: u64,
    /// 逻辑 CPU 核心数
    pub cpu_count: u32,
    /// CPU 型号
    pub cpu_model: String,
    /// CPU 当前频率（MHz）
    pub cpu_frequency_mhz: f64,
    /// CPU 缓存大小（字节）
    pub cpu_cache: u64,
    /// CPU BogoMips
    pub cpu_bogo_mips: f64,
    /// CPU 总体使用率（0-100）
    pub cpu_usage: f64,
    /// CPU 各类别占用率
    pub cpu_usage_breakdown: CpuUsageBreakdown,
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
    /// 完整文件系统列表（包含虚拟文件系统）
    pub file_systems: Vec<DiskUsage>,
    /// 进程列表（按 CPU 降序）
    pub processes: Vec<ProcessInfo>,
}

/// 将原始输出按 `###标记###` 切分为各区块
fn split_sections(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
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

/// `/proc/stat` 中参与 CPU 占用计算的累计时间
#[derive(Debug, Clone, Copy, Default)]
struct CpuTimes {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    io_wait: u64,
    irq: u64,
    soft_irq: u64,
    steal: u64,
}

/// 从 `/proc/stat` 首行解析 CPU 各类别累计时间
fn parse_stat(line: &str) -> CpuTimes {
    let nums: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|n| n.parse().ok())
        .collect();
    if nums.len() < 4 {
        return CpuTimes::default();
    }
    CpuTimes {
        user: nums[0],
        nice: nums[1],
        system: nums[2],
        idle: nums[3],
        io_wait: nums.get(4).copied().unwrap_or(0),
        irq: nums.get(5).copied().unwrap_or(0),
        soft_irq: nums.get(6).copied().unwrap_or(0),
        steal: nums.get(7).copied().unwrap_or(0),
    }
}

/// 根据两次 `/proc/stat` 采样计算 CPU 总占用率和各类别占比
fn calculate_cpu_usage(first: CpuTimes, second: CpuTimes) -> (f64, CpuUsageBreakdown) {
    let delta = CpuTimes {
        user: second.user.saturating_sub(first.user),
        nice: second.nice.saturating_sub(first.nice),
        system: second.system.saturating_sub(first.system),
        idle: second.idle.saturating_sub(first.idle),
        io_wait: second.io_wait.saturating_sub(first.io_wait),
        irq: second.irq.saturating_sub(first.irq),
        soft_irq: second.soft_irq.saturating_sub(first.soft_irq),
        steal: second.steal.saturating_sub(first.steal),
    };
    let total = delta.user
        + delta.nice
        + delta.system
        + delta.idle
        + delta.io_wait
        + delta.irq
        + delta.soft_irq
        + delta.steal;
    if total == 0 {
        return (0.0, CpuUsageBreakdown::default());
    }

    let percent = |value: u64| value as f64 / total as f64 * 100.0;
    let breakdown = CpuUsageBreakdown {
        user: percent(delta.user),
        system: percent(delta.system),
        nice: percent(delta.nice),
        idle: percent(delta.idle),
        io_wait: percent(delta.io_wait),
        irq: percent(delta.irq),
        soft_irq: percent(delta.soft_irq),
        steal: percent(delta.steal),
    };
    let busy = total.saturating_sub(delta.idle + delta.io_wait);
    (percent(busy).clamp(0.0, 100.0), breakdown)
}

/// 解析 /proc/net/dev，返回各网卡 (rx_bytes, tx_bytes)
fn parse_net(raw: &str) -> HashMap<String, (u64, u64)> {
    let mut map = HashMap::new();
    for line in raw.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
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

/// 从文本开头提取浮点数，兼容 `2300.00 MHz` 与 `2300MHz` 等格式
fn parse_leading_number(value: &str) -> f64 {
    value
        .trim()
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .parse()
        .unwrap_or(0.0)
}

/// 将 `/proc/cpuinfo` 的缓存大小转换为字节数
fn parse_cache_size(value: &str) -> u64 {
    let mut parts = value.split_whitespace();
    let amount = parts
        .next()
        .and_then(|part| part.parse::<f64>().ok())
        .unwrap_or(0.0);
    let multiplier = match parts
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (amount * multiplier) as u64
}

/// 从筛选后的处理器信息中提取 CPU 型号、频率、缓存和 BogoMips
fn parse_cpu_info(raw: &str) -> (String, f64, u64, f64) {
    let mut model = String::new();
    let mut frequency_mhz = 0.0;
    let mut cache = 0;
    let mut bogo_mips = 0.0;

    for line in raw.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            // 标准型号字段优先级最高，避免 x86 的数字 model 字段提前占位
            "model name" => {
                model = value.to_string();
            }
            "hardware" | "processor" | "model"
                if model.is_empty() && value.chars().any(|ch| ch.is_ascii_alphabetic()) =>
            {
                model = value.to_string();
            }
            "cpu mhz" if frequency_mhz == 0.0 => {
                frequency_mhz = parse_leading_number(value);
            }
            "clock" if frequency_mhz == 0.0 => {
                frequency_mhz = parse_leading_number(value);
                if value.to_ascii_lowercase().contains("ghz") {
                    frequency_mhz *= 1000.0;
                }
            }
            "cache size" if cache == 0 => {
                cache = parse_cache_size(value);
            }
            "bogomips" if bogo_mips == 0.0 => {
                bogo_mips = parse_leading_number(value);
            }
            _ => {}
        }
    }

    (model, frequency_mhz, cache, bogo_mips)
}

/// 解析 `df -kP` 输出，分别返回侧栏磁盘摘要与完整文件系统列表
fn parse_disks(raw: &str) -> (Vec<DiskUsage>, Vec<DiskUsage>) {
    let mut disks = Vec::new();
    let mut file_systems = Vec::new();

    for line in raw.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        let disk = DiskUsage {
            filesystem: fields[0].to_string(),
            mount: fields[5..].join(" "),
            total: fields[1].parse::<u64>().unwrap_or(0) * 1024,
            used: fields[2].parse::<u64>().unwrap_or(0) * 1024,
            available: fields[3].parse::<u64>().unwrap_or(0) * 1024,
            use_percent: fields[4]
                .trim_end_matches('%')
                .parse::<f64>()
                .unwrap_or(0.0),
        };
        file_systems.push(disk.clone());

        // 左侧监控保持精简，只展示实际磁盘分区
        if !disk.filesystem.starts_with("tmpfs")
            && !disk.filesystem.starts_with("devtmpfs")
            && disk.filesystem != "overlay"
        {
            disks.push(disk);
        }
    }

    (disks, file_systems)
}

/// 采集并解析一次监控数据
pub async fn collect(manager: &SessionManager, session_id: &str) -> Result<MonitorData> {
    let raw = timeout(MONITOR_TIMEOUT, manager.exec(session_id, MONITOR_SCRIPT))
        .await
        .map_err(|_| anyhow!("监控数据采集超时"))??;
    let sections = split_sections(&raw);
    let get = |k: &str| sections.get(k).cloned().unwrap_or_default();

    let mut data = MonitorData {
        hostname: get("HOST"),
        os: get("OS"),
        kernel_name: get("KERNELNAME"),
        kernel: get("KERNEL"),
        architecture: get("ARCH"),
        cpu_count: get("CPUCOUNT").trim().parse().unwrap_or(0),
        ..Default::default()
    };

    let (cpu_model, cpu_frequency_mhz, cpu_cache, cpu_bogo_mips) = parse_cpu_info(&get("CPUINFO"));
    data.cpu_model = cpu_model;
    data.cpu_frequency_mhz = cpu_frequency_mhz;
    data.cpu_cache = cpu_cache;
    data.cpu_bogo_mips = cpu_bogo_mips;

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
    let (cpu_usage, cpu_usage_breakdown) =
        calculate_cpu_usage(parse_stat(&get("STAT1")), parse_stat(&get("STAT2")));
    data.cpu_usage = cpu_usage;
    data.cpu_usage_breakdown = cpu_usage_breakdown;

    // 内存
    let mem = get("MEM");
    data.mem_total = mem_field(&mem, "MemTotal");
    data.mem_available = mem_field(&mem, "MemAvailable");
    data.mem_used = data.mem_total.saturating_sub(data.mem_available);
    data.swap_total = mem_field(&mem, "SwapTotal");
    let swap_free = mem_field(&mem, "SwapFree");
    data.swap_used = data.swap_total.saturating_sub(swap_free);

    // 网卡速率（优先采用远端单调时钟的实际采样间隔）
    let net1 = parse_net(&get("NET1"));
    let net2 = parse_net(&get("NET2"));
    // 物理网卡名集合
    let phys: HashSet<String> = get("PHYS")
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let net_time1 = parse_leading_number(&get("NETTIME1"));
    let net_time2 = parse_leading_number(&get("NETTIME2"));
    let interval = if net_time2 > net_time1 {
        net_time2 - net_time1
    } else {
        0.5
    };
    for (name, (rx2, tx2)) in &net2 {
        let (rx1, tx1) = net1.get(name).copied().unwrap_or((*rx2, *tx2));
        data.net_interfaces.push(NetInterface {
            name: name.clone(),
            rx_rate: ((rx2.saturating_sub(rx1)) as f64 / interval) as u64,
            tx_rate: ((tx2.saturating_sub(tx1)) as f64 / interval) as u64,
            rx_total: *rx2,
            tx_total: *tx2,
            is_physical: phys.contains(name),
        });
    }
    data.net_interfaces.sort_by(|a, b| a.name.cmp(&b.name));

    // 磁盘与完整文件系统
    (data.disks, data.file_systems) = parse_disks(&get("DISK"));

    // 进程（首行为表头）
    for line in get("PROC").lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        data.processes.push(ProcessInfo {
            pid: f[0].parse().unwrap_or(0),
            name: f[1].to_string(),
            cpu: f[2].parse().unwrap_or(0.0),
            mem: f[3].parse().unwrap_or(0.0),
            // ps 的 rss 单位为 KiB，转字节
            mem_bytes: f[4].parse::<u64>().unwrap_or(0) * 1024,
        });
    }

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 断言两个浮点数在监控精度范围内相等
    fn assert_approx_eq(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.001,
            "实际值 {actual}，期望值 {expected}"
        );
    }

    /// CPU 占用计算应忽略 guest 重复字段并返回各类别占比
    #[test]
    fn calculates_cpu_usage_breakdown() {
        let first = parse_stat("cpu 100 20 30 400 10 5 15 20 99 88");
        let second = parse_stat("cpu 130 25 50 440 20 10 25 30 199 188");

        let (usage, breakdown) = calculate_cpu_usage(first, second);

        assert_approx_eq(usage, 61.538_461_538);
        assert_approx_eq(breakdown.user, 23.076_923_077);
        assert_approx_eq(breakdown.system, 15.384_615_385);
        assert_approx_eq(breakdown.nice, 3.846_153_846);
        assert_approx_eq(breakdown.idle, 30.769_230_769);
        assert_approx_eq(breakdown.io_wait, 7.692_307_692);
        assert_approx_eq(breakdown.irq, 3.846_153_846);
        assert_approx_eq(breakdown.soft_irq, 7.692_307_692);
        assert_approx_eq(breakdown.steal, 7.692_307_692);
    }

    /// 两次 CPU 采样无变化时应稳定返回零值
    #[test]
    fn handles_unchanged_cpu_sample() {
        let sample = parse_stat("cpu 100 20 30 400 10 5 15 20");

        let (usage, breakdown) = calculate_cpu_usage(sample, sample);

        assert_eq!(usage, 0.0);
        assert_eq!(breakdown.user, 0.0);
        assert_eq!(breakdown.idle, 0.0);
    }

    /// CPU 信息解析应兼容常见的 `/proc/cpuinfo` 字段格式
    #[test]
    fn parses_cpu_information() {
        let raw = r#"processor : 0
model : 186
model name : 13th Gen Intel(R) Core(TM) i5-13500H
cpu MHz : 4533.129
cache size : 18432 KB
bogomips : 6374.42"#;

        let (model, frequency_mhz, cache, bogo_mips) = parse_cpu_info(raw);

        assert_eq!(model, "13th Gen Intel(R) Core(TM) i5-13500H");
        assert_approx_eq(frequency_mhz, 4533.129);
        assert_eq!(cache, 18_874_368);
        assert_approx_eq(bogo_mips, 6374.42);
    }

    /// CPU 信息缺少可选字段时应保留型号并让数值字段回退为零
    #[test]
    fn parses_partial_arm_cpu_information() {
        let raw = "processor : 0\nHardware : BCM2711\n";

        let (model, frequency_mhz, cache, bogo_mips) = parse_cpu_info(raw);

        assert_eq!(model, "BCM2711");
        assert_eq!(frequency_mhz, 0.0);
        assert_eq!(cache, 0);
        assert_eq!(bogo_mips, 0.0);
    }

    /// 网络解析应保留回环接口并读取累计收发字节数
    #[test]
    fn parses_loopback_network_interface() {
        let raw = "Inter-| Receive | Transmit\n lo: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0";

        let interfaces = parse_net(raw);

        assert_eq!(interfaces.get("lo"), Some(&(100, 200)));
    }

    /// 完整文件系统应保留虚拟挂载，侧栏摘要仍只包含实际磁盘
    #[test]
    fn separates_sidebar_disks_from_all_file_systems() {
        let raw = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                   tmpfs 100 10 90 10% /run\n\
                   overlay 200 40 160 20% /var/lib/docker/overlay2/merged\n\
                   /dev/sda1 1000 250 750 25% /";

        let (disks, file_systems) = parse_disks(raw);

        assert_eq!(file_systems.len(), 3);
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].filesystem, "/dev/sda1");
    }
}
