//! 远程系统监控数据采集与解析
//!
//! 通过一条自包含的远程命令读取 `/proc` 与少量 `df`/`ps` 输出，
//! 在 Rust 侧按相邻采样计算 CPU 与网卡速率，避免远端驻留服务与采样等待

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Serialize;
use tokio::time::timeout;

use super::manager::SessionManager;
use super::process::collector_process_ids;

/// 单次监控采集允许的最长时间
const MONITOR_TIMEOUT: Duration = Duration::from_secs(15);

/// 采集监控数据的远程命令。高频数据只取一次累计值，由 Rust 按相邻轮次计算速率
const MONITOR_SCRIPT: &str = r####"
LC_ALL=C; export LC_ALL
: __ZT_MONITOR_COLLECTOR__
printf 'ztshell-mon' > /proc/self/comm 2>/dev/null || true
printf '###MONITOR_META###\n%s\n' "$$"
awk -F: '
function print_file(path, line) {
  while ((getline line < path) > 0) print line
  close(path)
}
function print_first(path, line) {
  if ((getline line < path) > 0) print line
  close(path)
}
function count_cpu_range(spec, ranges, parts, count, range_count, part_count, idx) {
  count = 0
  range_count = split(spec, ranges, ",")
  for (idx = 1; idx <= range_count; idx++) {
    part_count = split(ranges[idx], parts, "-")
    if (part_count == 2) count += parts[2] - parts[1] + 1
    else if (parts[1] != "") count++
  }
  return count
}
BEGIN {
  print "###HOST###"
  print_first("/proc/sys/kernel/hostname")

  print "###OS###"
  while ((getline line < "/etc/os-release") > 0) {
    if (line ~ /^PRETTY_NAME=/) {
      sub(/^PRETTY_NAME=/, "", line)
      if (line ~ /^\".*\"$/) {
        sub(/^\"/, "", line)
        sub(/\"$/, "", line)
      }
      print line
      break
    }
  }
  close("/etc/os-release")

  print "###KERNELNAME###"
  print_first("/proc/sys/kernel/ostype")
  print "###KERNEL###"
  print_first("/proc/sys/kernel/osrelease")
  print "###UPTIME###"
  uptime_ok = (getline uptime_line < "/proc/uptime") > 0
  if (uptime_ok) print uptime_line
  close("/proc/uptime")

  cpu_count = 0
  cpu_info_count = 0
  while ((getline cpu_line < "/proc/cpuinfo") > 0) {
    split(cpu_line, pair, ":")
    key = tolower(pair[1])
    gsub(/^[ \t]+|[ \t]+$/, "", key)
    value = pair[2]
    if (key == "processor" && value !~ /[[:alpha:]]/) {
      cpu_count++
      continue
    }
    if ((key == "model name" || key == "hardware" || key == "model" || key == "processor" ||
         key == "cpu implementer" || key == "cpu part" || key == "cpu mhz" || key == "clock" ||
         key == "cache size" || key == "bogomips") && !seen[key]++) {
      cpu_info[++cpu_info_count] = cpu_line
    }
  }
  close("/proc/cpuinfo")
  if (cpu_count == 0 && (getline online < "/sys/devices/system/cpu/online") > 0) {
    cpu_count = count_cpu_range(online)
  }
  close("/sys/devices/system/cpu/online")
  print "###CPUCOUNT###"
  print cpu_count
  print "###CPUINFO###"
  for (idx = 1; idx <= cpu_info_count; idx++) print cpu_info[idx]

  print "###LOADAVG###"
  print_first("/proc/loadavg")
  print "###MEM###"
  print_file("/proc/meminfo")
  print "###STAT###"
  stat_ok = (getline stat_line < "/proc/stat") > 0
  if (stat_ok) print stat_line
  close("/proc/stat")
  print "###NET###"
  net_line_count = 0
  while ((getline net_line < "/proc/net/dev") > 0) {
    print net_line
    net_line_count++
  }
  close("/proc/net/dev")
  print "###NETTIME###"
  print uptime_line
  if (!uptime_ok || !stat_ok || net_line_count == 0) exit 1
}' </dev/null 2>/dev/null || {
  printf '###ERROR###\n'
  exit 1
}
printf '###ARCH###\n'; uname -m 2>/dev/null
printf '###BOARD###\n'
if [ -r /sys/firmware/devicetree/base/model ]; then
  tr -d '\000' < /sys/firmware/devicetree/base/model 2>/dev/null
  printf '\n'
fi
printf '###COMPATIBLE###\n'
if [ -r /sys/firmware/devicetree/base/compatible ]; then
  tr '\000' '\n' < /sys/firmware/devicetree/base/compatible 2>/dev/null
fi
printf '###CPUFREQ###\n'
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq 2>/dev/null || true
printf '###CPUCACHE###\n'
for path in /sys/devices/system/cpu/cpu0/cache/index*/size; do
  [ -r "$path" ] && cat "$path"
done 2>/dev/null
printf '###CLOCK_TICKS###\n'
getconf CLK_TCK 2>/dev/null || printf '100\n'
printf '###PHYS###\n'
for path in /sys/class/net/*; do
  [ -e "$path/device" ] && printf '%s\n' "${path##*/}"
done 2>/dev/null
printf '###DISK###\n'; df -kP 2>/dev/null
printf '###PROC###\n'
ps -ww -e \
  -o pid= -o ppid= -o pmem= -o rss= -o args= 2>/dev/null |
awk '
function next_field(value) {
  sub(/^[[:space:]]*/, "", remaining)
  value = remaining
  sub(/[[:space:]].*$/, "", value)
  sub(/^[^[:space:]]+/, "", remaining)
  return value
}
{
  remaining = $0
  pid = next_field()
  ppid = next_field()
  mem = next_field()
  rss = next_field()
  sub(/^[[:space:]]*/, "", remaining)
  command = remaining

  cpu_ticks = 0
  start_time = 0
  stat_path = "/proc/" pid "/stat"
  if ((getline stat_line < stat_path) > 0) {
    sub(/^.*\) /, "", stat_line)
    split(stat_line, stat_fields, /[[:space:]]+/)
    cpu_ticks = stat_fields[12] + stat_fields[13]
    start_time = stat_fields[20]
  }
  close(stat_path)

  name = ""
  comm_path = "/proc/" pid "/comm"
  if ((getline name < comm_path) <= 0) name = ""
  close(comm_path)
  collector = (name == "ztshell-mon" && command ~ /__ZT_MONITOR_COLLECTOR__/) ||
              (name == "ztshell-proc" && command ~ /__ZT_PROCESS_COLLECTOR__/)
  printf "%s\037%s\037%s\037%s\037%s\037%s\037%d\037%s\n", \
    pid, ppid, mem, rss, cpu_ticks, start_time, collector, name
}'
printf '###END###\n'
"####;

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
    /// 设备树中的主板型号
    pub board_model: String,
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

impl CpuTimes {
    /// 返回参与占用率计算的累计时钟总量
    fn total(self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.io_wait
            + self.irq
            + self.soft_irq
            + self.steal
    }
}

/// 单个进程的累计 CPU 时间快照
#[derive(Debug, Clone, Copy)]
struct ProcessCpuSnapshot {
    start_time: u64,
    cpu_ticks: u64,
}

/// 一轮远端采样中用于下一轮差值计算的累计值
#[derive(Debug, Clone, Default)]
struct MonitorSnapshot {
    cpu: CpuTimes,
    net: HashMap<String, (u64, u64)>,
    process_cpu: HashMap<u32, ProcessCpuSnapshot>,
    timestamp: f64,
}

/// 绑定到 SSH 会话的监控运行时状态
#[derive(Debug, Default)]
pub(crate) struct MonitorRuntimeState {
    previous: Option<MonitorSnapshot>,
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
    let total = delta.total();
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
    let amount_text = parts.next().unwrap_or_default();
    let amount = parse_leading_number(amount_text);
    let suffix = amount_text
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit() && *ch != '.')
        .map(|(index, _)| &amount_text[index..])
        .or_else(|| parts.next())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let multiplier = match suffix.as_str() {
        "kb" | "kib" | "k" => 1024.0,
        "mb" | "mib" | "m" => 1024.0 * 1024.0,
        "gb" | "gib" | "g" => 1024.0 * 1024.0 * 1024.0,
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
    let mut implementer = String::new();
    let mut part = String::new();

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
            "cpu implementer" if implementer.is_empty() => {
                implementer = value.to_ascii_lowercase();
            }
            "cpu part" if part.is_empty() => {
                part = value.to_ascii_lowercase();
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

    if model.is_empty() {
        // ARM64 设备通常只提供 implementer/part，不能依赖 model name 字段。
        model = match (implementer.as_str(), part.as_str()) {
            ("0x41", "0xd03") => "ARM Cortex-A53".to_string(),
            ("0x41", "0xd05") => "ARM Cortex-A55".to_string(),
            ("0x41", "0xd0a") => "ARM Cortex-A75".to_string(),
            ("0x41", "0xd0b") => "ARM Cortex-A76".to_string(),
            ("0x41", "0xd0d") => "ARM Cortex-A77".to_string(),
            ("0x41", "0xd41") => "ARM Cortex-X1".to_string(),
            ("0x41", "0xd46") => "ARM Cortex-A510".to_string(),
            ("0x41", "0xd47") => "ARM Cortex-A710".to_string(),
            _ if !implementer.is_empty() || !part.is_empty() => {
                format!("ARM implementer {implementer} part {part}")
            }
            _ => String::new(),
        };
    }

    (model, frequency_mhz, cache, bogo_mips)
}

/// 解析 sysfs 中以 kHz 表示的当前频率，并转换为 MHz
fn parse_cpu_frequency(raw: &str) -> f64 {
    let value = parse_leading_number(raw);
    if value > 100_000.0 {
        value / 1000.0
    } else {
        value
    }
}

/// 从多个缓存层级中选择容量最大的一级
fn parse_cpu_cache(raw: &str) -> u64 {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(parse_cache_size)
        .max()
        .unwrap_or(0)
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

/// 侧栏进程快照解析时使用的父子关系与资源数据
struct MonitorProcessRow {
    pid: u32,
    ppid: u32,
    is_collector: bool,
    name: String,
    cpu_ticks: u64,
    start_time: u64,
    mem: f64,
    mem_bytes: u64,
}

/// 解析采集脚本自身的进程标识
fn parse_monitor_roots(raw: &str) -> Vec<u32> {
    raw.split_whitespace()
        .filter_map(|value| value.parse().ok())
        .filter(|pid| *pid > 0)
        .take(1)
        .collect()
}

/// 解析进程累计 CPU 时间，避免使用 ps 的进程生命周期平均值
fn parse_monitor_processes(raw: &str) -> Vec<MonitorProcessRow> {
    raw.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.splitn(8, '\u{1f}').collect();
            if fields.len() != 8 {
                return None;
            }
            let pid = fields[0].trim().parse().ok()?;
            if pid == 0 {
                return None;
            }
            Some(MonitorProcessRow {
                pid,
                ppid: fields[1].trim().parse().unwrap_or(0),
                mem: fields[2].trim().parse().unwrap_or(0.0),
                mem_bytes: fields[3].trim().parse::<u64>().unwrap_or(0) * 1024,
                cpu_ticks: fields[4].trim().parse().unwrap_or(0),
                start_time: fields[5].trim().parse().unwrap_or(0),
                is_collector: fields[6].trim() == "1",
                name: fields[7].trim().to_string(),
            })
        })
        .collect()
}

/// 将一轮远端原始输出与上一轮累计值合成为可展示数据
fn parse_monitor_data(
    raw: &str,
    previous: Option<&MonitorSnapshot>,
) -> Result<(MonitorData, MonitorSnapshot)> {
    let sections = split_sections(raw);
    if sections.contains_key("ERROR") || !sections.contains_key("END") {
        return Err(anyhow!("远端监控核心数据返回不完整"));
    }
    let get = |key: &str| sections.get(key).map(String::as_str).unwrap_or_default();

    let mut data = MonitorData {
        hostname: get("HOST").to_string(),
        os: get("OS").to_string(),
        kernel_name: get("KERNELNAME").to_string(),
        kernel: get("KERNEL").to_string(),
        architecture: get("ARCH").to_string(),
        board_model: get("BOARD").trim().to_string(),
        cpu_count: get("CPUCOUNT").trim().parse().unwrap_or(0),
        ..Default::default()
    };
    if data.board_model.is_empty() {
        data.board_model = get("COMPATIBLE")
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_string();
    }

    let (cpu_model, cpu_info_frequency_mhz, cpu_info_cache, cpu_bogo_mips) =
        parse_cpu_info(get("CPUINFO"));
    data.cpu_model = cpu_model;
    data.cpu_frequency_mhz = parse_cpu_frequency(get("CPUFREQ"));
    if data.cpu_frequency_mhz == 0.0 {
        data.cpu_frequency_mhz = cpu_info_frequency_mhz;
    }
    data.cpu_cache = parse_cpu_cache(get("CPUCACHE"));
    if data.cpu_cache == 0 {
        data.cpu_cache = cpu_info_cache;
    }
    data.cpu_bogo_mips = cpu_bogo_mips;

    let timestamp = get("NETTIME")
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| anyhow!("远端监控时间数据无效"))?;
    data.uptime = parse_leading_number(get("UPTIME")) as u64;

    let load: Vec<f64> = get("LOADAVG")
        .split_whitespace()
        .take(3)
        .filter_map(|value| value.parse().ok())
        .collect();
    for (index, value) in load.into_iter().enumerate().take(3) {
        data.load_avg[index] = value;
    }

    let cpu = parse_stat(get("STAT"));
    if cpu.total() == 0 {
        return Err(anyhow!("远端 CPU 累计数据无效"));
    }
    if let Some(previous) = previous.filter(|sample| timestamp > sample.timestamp) {
        (data.cpu_usage, data.cpu_usage_breakdown) = calculate_cpu_usage(previous.cpu, cpu);
    }

    let mem = get("MEM");
    data.mem_total = mem_field(mem, "MemTotal");
    if data.mem_total == 0 {
        return Err(anyhow!("远端内存数据无效"));
    }
    data.mem_available = mem_field(mem, "MemAvailable");
    data.mem_used = data.mem_total.saturating_sub(data.mem_available);
    data.swap_total = mem_field(mem, "SwapTotal");
    let swap_free = mem_field(mem, "SwapFree");
    data.swap_used = data.swap_total.saturating_sub(swap_free);

    let net = parse_net(get("NET"));
    if net.is_empty() {
        return Err(anyhow!("远端网卡累计数据无效"));
    }
    let phys: HashSet<String> = get("PHYS")
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    let interval = previous
        .filter(|sample| timestamp > sample.timestamp)
        .map(|sample| timestamp - sample.timestamp)
        .unwrap_or(0.0);
    for (name, (rx_total, tx_total)) in &net {
        let (rx_rate, tx_rate) = previous
            .and_then(|sample| sample.net.get(name))
            .filter(|_| interval > 0.0)
            .map(|(previous_rx, previous_tx)| {
                (
                    (rx_total.saturating_sub(*previous_rx) as f64 / interval) as u64,
                    (tx_total.saturating_sub(*previous_tx) as f64 / interval) as u64,
                )
            })
            .unwrap_or((0, 0));
        data.net_interfaces.push(NetInterface {
            name: name.clone(),
            rx_rate,
            tx_rate,
            rx_total: *rx_total,
            tx_total: *tx_total,
            is_physical: phys.contains(name),
        });
    }
    data.net_interfaces
        .sort_by(|left, right| left.name.cmp(&right.name));

    (data.disks, data.file_systems) = parse_disks(get("DISK"));

    let process_rows = parse_monitor_processes(get("PROC"));
    let excluded = collector_process_ids(
        process_rows
            .iter()
            .map(|process| (process.pid, process.ppid, process.is_collector)),
        parse_monitor_roots(get("MONITOR_META")),
    );
    let clock_ticks = get("CLOCK_TICKS")
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(100);
    let current_process_cpu: HashMap<u32, ProcessCpuSnapshot> = process_rows
        .iter()
        .map(|process| {
            (
                process.pid,
                ProcessCpuSnapshot {
                    start_time: process.start_time,
                    cpu_ticks: process.cpu_ticks,
                },
            )
        })
        .collect();
    let mut processes = process_rows
        .into_iter()
        .filter(|process| !excluded.contains(&process.pid))
        .map(|process| {
            let cpu = previous
                .filter(|sample| interval > 0.0)
                .and_then(|sample| sample.process_cpu.get(&process.pid))
                .filter(|previous| previous.start_time == process.start_time)
                .map(|previous| {
                    process.cpu_ticks.saturating_sub(previous.cpu_ticks) as f64
                        / clock_ticks as f64
                        / interval
                        * 100.0
                })
                .unwrap_or(0.0);
            ProcessInfo {
                pid: process.pid,
                name: process.name,
                cpu,
                mem: process.mem,
                mem_bytes: process.mem_bytes,
            }
        })
        .collect::<Vec<_>>();
    processes.sort_by(|left, right| right.cpu.total_cmp(&left.cpu));
    processes.truncate(15);
    data.processes = processes;

    Ok((
        data,
        MonitorSnapshot {
            cpu,
            net,
            process_cpu: current_process_cpu,
            timestamp,
        },
    ))
}

/// 采集并解析一次监控数据
pub async fn collect(manager: &SessionManager, session_id: &str) -> Result<MonitorData> {
    let state = manager.monitor_state(session_id)?;
    let mut state = state.lock().await;
    let raw = timeout(MONITOR_TIMEOUT, manager.exec(session_id, MONITOR_SCRIPT))
        .await
        .map_err(|_| anyhow!("监控数据采集超时"))??;
    let (data, snapshot) = parse_monitor_data(&raw, state.previous.as_ref())?;
    state.previous = Some(snapshot);
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

    /// ARM64 常见的 cpuinfo 缺少 model name 时应根据 implementer/part 推导型号
    #[test]
    fn derives_arm_cpu_model_from_implementer_and_part() {
        let raw = "processor : 0\nCPU implementer : 0x41\nCPU part : 0xd05\nBogoMIPS : 48.00\n";

        let (model, _, _, bogo_mips) = parse_cpu_info(raw);

        assert_eq!(model, "ARM Cortex-A55");
        assert_approx_eq(bogo_mips, 48.0);
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

    /// sysfs CPU 频率与缓存应使用监控数据中的统一单位
    #[test]
    fn parses_sysfs_cpu_properties() {
        assert_approx_eq(parse_cpu_frequency("1800000\n"), 1800.0);
        assert_eq!(parse_cpu_cache("32K\n32K\n512K\n"), 512 * 1024);
    }

    /// 网络解析应保留回环接口并读取累计收发字节数
    #[test]
    fn parses_loopback_network_interface() {
        let raw = "Inter-| Receive | Transmit\n lo: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0";

        let interfaces = parse_net(raw);

        assert_eq!(interfaces.get("lo"), Some(&(100, 200)));
    }

    /// 相邻采样应计算 CPU 与网卡速率，并排除采集器产生的进程
    #[test]
    fn calculates_rates_across_monitor_samples() {
        let first_raw = "###UPTIME###\n10.00 0.00\n\
                         ###NETTIME###\n10.00 0.00\n\
                         ###STAT###\ncpu 100 0 100 800 0 0 0 0\n\
                         ###MEM###\nMemTotal: 1000 kB\nMemAvailable: 600 kB\n\
                         ###NET###\neth0: 1000 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0\n\
                         ###PHYS###\neth0\n\
                         ###MONITOR_META###\n900\n\
                         ###CLOCK_TICKS###\n100\n\
                         ###PROC###\n900\x1f800\x1f0.1\x1f10\x1f300\x1f4\x1f1\x1fztshell-mon\n\
                         901\x1f900\x1f0.1\x1f10\x1f200\x1f5\x1f0\x1fps\n\
                         42\x1f1\x1f1.5\x1f2048\x1f12\x1f5\x1f0\x1fapp worker\n\
                         ###END###\n";
        let (first, first_snapshot) = parse_monitor_data(first_raw, None).unwrap();

        assert_eq!(first.cpu_usage, 0.0);
        assert_eq!(first.net_interfaces[0].rx_rate, 0);
        assert_eq!(first.processes.len(), 1);
        assert_eq!(first.processes[0].pid, 42);
        assert_eq!(first.processes[0].name, "app worker");

        let second_raw = "###UPTIME###\n13.00 0.00\n\
                          ###NETTIME###\n13.00 0.00\n\
                          ###STAT###\ncpu 120 0 130 850 0 0 0 0\n\
                          ###MEM###\nMemTotal: 1000 kB\nMemAvailable: 600 kB\n\
                          ###NET###\neth0: 1300 0 0 0 0 0 0 0 2600 0 0 0 0 0 0 0\n\
                          ###PHYS###\neth0\n\
                          ###CLOCK_TICKS###\n100\n\
                          ###PROC###\n42\x1f1\x1f1.5\x1f2048\x1f42\x1f5\x1f0\x1fapp worker\n\
                          ###END###\n";
        let (second, _) = parse_monitor_data(second_raw, Some(&first_snapshot)).unwrap();

        assert_approx_eq(second.cpu_usage, 50.0);
        assert_approx_eq(second.cpu_usage_breakdown.user, 20.0);
        assert_approx_eq(second.cpu_usage_breakdown.system, 30.0);
        assert_eq!(second.processes[0].cpu, 10.0);
        assert_eq!(second.net_interfaces[0].rx_rate, 100);
        assert_eq!(second.net_interfaces[0].tx_rate, 200);
    }

    /// 缺少核心累计值的采样必须报错，调用方因而不会覆盖上一轮基线
    #[test]
    fn rejects_sample_without_cpu_counters() {
        let raw = "###UPTIME###\n13.00 0.00\n\
                   ###NETTIME###\n13.00 0.00\n\
                   ###MEM###\nMemTotal: 1000 kB\nMemAvailable: 600 kB\n\
                   ###NET###\nlo: 100 0 0 0 0 0 0 0 100 0 0 0 0 0 0 0\n\
                   ###END###\n";

        let error = parse_monitor_data(raw, None).unwrap_err();

        assert_eq!(error.to_string(), "远端 CPU 累计数据无效");
    }

    /// 高频采样脚本不得在远端等待第二次 CPU 或网络读数
    #[test]
    fn monitor_script_uses_single_snapshot() {
        assert!(!MONITOR_SCRIPT.contains("sleep "));
        assert!(!MONITOR_SCRIPT.contains("STAT1"));
        assert!(!MONITOR_SCRIPT.contains("NET1"));
        assert!(!MONITOR_SCRIPT.contains("head "));
        assert!(!MONITOR_SCRIPT.contains("--sort=-pcpu"));
        assert!(MONITOR_SCRIPT.contains("/proc/"));
        assert!(MONITOR_SCRIPT.contains("cpu implementer"));
        assert!(MONITOR_SCRIPT.contains("cpu part"));
        assert!(MONITOR_SCRIPT.contains("###BOARD###"));
        assert!(MONITOR_SCRIPT.contains("###CPUFREQ###"));
        assert!(MONITOR_SCRIPT.contains("###ERROR###"));
        assert!(MONITOR_SCRIPT.contains("__ZT_MONITOR_COLLECTOR__"));
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
