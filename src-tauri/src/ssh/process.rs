//! 远端 Linux 进程列表、详情与操作

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Serialize;
use tokio::time::timeout;

use super::manager::SessionManager;

/// 单次进程查询或操作允许的最长时间
const PROCESS_TIMEOUT: Duration = Duration::from_secs(15);
/// 进程列表脚本执行失败标记
const LIST_ERROR_MARKER: &str = "__ZT_PROCESS_LIST_ERROR__";
/// 进程列表脚本采集器元数据前缀
const LIST_META_PREFIX: &str = "__ZT_PROCESS_META__";
/// 进程列表脚本可执行路径区块标记
const LIST_EXE_MARKER: &str = "__ZT_PROCESS_EXE__";
/// 进程列表脚本完整结束标记
const LIST_DONE_MARKER: &str = "__ZT_PROCESS_LIST_DONE__";
/// 进程详情查询时目标已失效的标记
const DETAIL_GONE_MARKER: &str = "__ZT_PROCESS_DETAIL_GONE__";
/// 进程详情脚本完整结束标记
const DETAIL_DONE_MARKER: &str = "__ZT_PROCESS_DETAIL_DONE__";
/// 进程终止成功标记
const TERMINATE_DONE_MARKER: &str = "__ZT_PROCESS_TERMINATE_DONE__";
/// 进程终止时目标已失效的标记
const TERMINATE_GONE_MARKER: &str = "__ZT_PROCESS_TERMINATE_GONE__";
/// 进程终止被远端系统拒绝的标记
const TERMINATE_DENIED_MARKER: &str = "__ZT_PROCESS_TERMINATE_DENIED__";

/// 查询完整进程列表的远端脚本
const PROCESS_LIST_SCRIPT: &str = r#"
LC_ALL=C; export LC_ALL
: __ZT_PROCESS_COLLECTOR__
printf 'ztshell-proc' > /proc/self/comm 2>/dev/null || true
collector_pid=$$
printf '__ZT_PROCESS_META__\037%s\n' "$collector_pid"
# procps-ng 4.x 不支持 --delimiter，命令行必须放在最后一列以保留其中的空格。
raw="$(ps -ww -e --sort=-pcpu \
  -o pid= -o ppid= -o user= -o rss= -o pcpu= -o args= 2>/dev/null)" || {
  printf '__ZT_PROCESS_LIST_ERROR__\n'
  exit
}

printf '%s\n' "$raw" |
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
  user = next_field()
  rss = next_field()
  cpu = next_field()
  sub(/^[[:space:]]*/, "", remaining)
  command = remaining

  start_time = 0
  stat_path = "/proc/" pid "/stat"
  if ((getline stat_line < stat_path) > 0) {
    sub(/^.*\) /, "", stat_line)
    split(stat_line, stat_fields, /[[:space:]]+/)
    start_time = stat_fields[20]
  }
  close(stat_path)

  name = ""
  comm_path = "/proc/" pid "/comm"
  if ((getline name < comm_path) <= 0) name = ""
  close(comm_path)

  printf "%s\037%s\037%s\037%s\037%s\037%s\037%s\037%s\n", \
    pid, ppid, user, rss, cpu, start_time, name, command
}' || {
  printf '__ZT_PROCESS_LIST_ERROR__\n'
  exit
}

printf '__ZT_PROCESS_EXE__\n'
if find /proc/self/exe -maxdepth 0 -printf '' >/dev/null 2>&1; then
  find /proc -mindepth 2 -maxdepth 2 -path '/proc/[0-9]*/exe' \
    -printf '%h\037%l\n' 2>/dev/null |
  awk -F '\037' '
  {
    process_path = $1
    executable = substr($0, length(process_path) + 2)
    pid = process_path
    sub(/^\/proc\//, "", pid)

    start_time = 0
    stat_path = "/proc/" pid "/stat"
    if ((getline stat_line < stat_path) > 0) {
      sub(/^.*\) /, "", stat_line)
      split(stat_line, stat_fields, /[[:space:]]+/)
      start_time = stat_fields[20]
    }
    close(stat_path)
    printf "%s\037%s\037%s\n", pid, start_time, executable
  }' || true
else
  for proc_dir in /proc/[0-9]*; do
    pid=${proc_dir#/proc/}
    start_time=0
    if IFS= read -r stat_line 2>/dev/null < "$proc_dir/stat"; then
      stat_rest=${stat_line##*) }
      set -- $stat_rest
      start_time=${20:-0}
    fi
    executable="$(readlink "$proc_dir/exe" 2>/dev/null || true)"
    printf '%s\037%s\037%s\n' "$pid" "$start_time" "$executable"
  done
fi
printf '__ZT_PROCESS_LIST_DONE__\n'
"#;

/// 查询单个进程详情的远端脚本模板
const PROCESS_DETAIL_SCRIPT: &str = r#"
LC_ALL=C; export LC_ALL
pid=__PID__
expected_start=__START_TIME__

current_start=0
if IFS= read -r stat_line < "/proc/$pid/stat"; then
  stat_rest=${stat_line##*) }
  set -- $stat_rest
  current_start=${20:-0}
fi
if [ "$current_start" = 0 ] || [ "$current_start" != "$expected_start" ]; then
  printf '__ZT_PROCESS_DETAIL_GONE__\n'
  exit
fi

hex_file() {
  od -An -v -tx1 "$1" 2>/dev/null | tr -d ' \n'
}
hex_link() {
  readlink "$1" 2>/dev/null | od -An -v -tx1 | tr -d ' \n'
}

printf 'NAME='
hex_file "/proc/$pid/comm"
printf '\nCOMMAND='
hex_file "/proc/$pid/cmdline"
printf '\nEXECUTABLE='
hex_link "/proc/$pid/exe"
printf '\nWORKING_DIRECTORY='
hex_link "/proc/$pid/cwd"
printf '\nENVIRONMENT='
hex_file "/proc/$pid/environ"
printf '\n__ZT_PROCESS_DETAIL_DONE__\n'
"#;

/// 向单个进程发送 SIGTERM 的远端脚本模板
const PROCESS_TERMINATE_SCRIPT: &str = r#"
pid=__PID__
expected_start=__START_TIME__

current_start=0
if IFS= read -r stat_line < "/proc/$pid/stat"; then
  stat_rest=${stat_line##*) }
  set -- $stat_rest
  current_start=${20:-0}
fi
if [ "$current_start" = 0 ] || [ "$current_start" != "$expected_start" ]; then
  printf '__ZT_PROCESS_TERMINATE_GONE__\n'
elif kill -TERM "$pid" 2>/dev/null; then
  printf '__ZT_PROCESS_TERMINATE_DONE__\n'
else
  printf '__ZT_PROCESS_TERMINATE_DENIED__\n'
fi
"#;

/// 进程列表条目
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListItem {
    /// 进程 ID
    pub pid: u32,
    /// 有效用户名称
    pub user: String,
    /// 实际内存占用（字节，来自 RSS）
    pub mem_bytes: u64,
    /// CPU 占用百分比
    pub cpu: f64,
    /// `/proc/<pid>/stat` 中的启动时钟值，用于识别 PID 是否已被复用
    pub start_time: u64,
    /// 进程名称
    pub name: String,
    /// 可执行文件位置，无权限读取时为空
    pub executable: String,
    /// 完整命令行
    pub command: String,
}

/// 进程环境变量
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEnvironmentVariable {
    /// 变量名
    pub name: String,
    /// 变量值
    pub value: String,
}

/// 单个进程的详细信息
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessDetail {
    /// 进程 ID
    pub pid: u32,
    /// 进程名称
    pub name: String,
    /// 完整命令行
    pub command: String,
    /// 可执行文件位置
    pub executable: String,
    /// 当前工作目录
    pub working_directory: String,
    /// 进程启动时的环境变量
    pub environment: Vec<ProcessEnvironmentVariable>,
}

/// 进程列表解析阶段使用的原始条目
struct RawProcessListItem {
    pid: u32,
    ppid: u32,
    user: String,
    mem_bytes: u64,
    cpu: f64,
    start_time: u64,
    name: String,
    command: String,
}

/// 判断进程名称与命令行是否共同匹配 ZTShell 采集器标记
fn is_collector_process(name: &str, command: &str) -> bool {
    match name {
        "ztshell-mon" => command.contains("__ZT_MONITOR_COLLECTOR__"),
        "ztshell-proc" => command.contains("__ZT_PROCESS_COLLECTOR__"),
        _ => false,
    }
}

/// 找出 ZTShell 采集器进程及其全部子进程
pub(crate) fn collector_process_ids(
    processes: impl IntoIterator<Item = (u32, u32, bool)>,
    explicit_roots: impl IntoIterator<Item = u32>,
) -> HashSet<u32> {
    let rows: Vec<(u32, u32, bool)> = processes.into_iter().collect();
    let mut excluded: HashSet<u32> = explicit_roots.into_iter().filter(|pid| *pid > 1).collect();
    for (pid, _, is_collector) in &rows {
        if *is_collector {
            excluded.insert(*pid);
        }
    }

    loop {
        let mut changed = false;
        for (pid, ppid, _) in &rows {
            if excluded.contains(ppid) && excluded.insert(*pid) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    excluded
}

/// 将列表脚本输出解析为结构化进程条目
fn parse_process_list(raw: &str) -> Result<Vec<ProcessListItem>> {
    if raw.lines().any(|line| line.trim() == LIST_ERROR_MARKER) {
        return Err(anyhow!("远端系统无法读取进程列表"));
    }
    if !raw.lines().any(|line| line.trim() == LIST_DONE_MARKER) {
        return Err(anyhow!("远端进程列表返回不完整"));
    }

    let mut raw_processes = Vec::new();
    let mut executables: HashMap<(u32, u64), String> = HashMap::new();
    let mut collector_roots = Vec::new();
    let mut reading_executables = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == LIST_DONE_MARKER {
            break;
        }
        if trimmed == LIST_EXE_MARKER {
            reading_executables = true;
            continue;
        }
        if let Some(meta) = line.strip_prefix(LIST_META_PREFIX) {
            collector_roots.extend(
                meta.split('\u{1f}')
                    .filter_map(|value| value.trim().parse::<u32>().ok())
                    .take(1),
            );
            continue;
        }
        if reading_executables {
            let fields: Vec<&str> = line.splitn(3, '\u{1f}').collect();
            if fields.len() != 3 {
                continue;
            }
            let pid = fields[0].trim().trim_start_matches("/proc/").parse::<u32>();
            let start_time = fields[1].trim().parse::<u64>();
            if let (Ok(pid), Ok(start_time)) = (pid, start_time) {
                if start_time > 0 {
                    executables.insert((pid, start_time), fields[2].to_string());
                }
            }
            continue;
        }

        let fields: Vec<&str> = line.splitn(8, '\u{1f}').collect();
        if fields.len() != 8 {
            continue;
        }
        let Ok(pid) = fields[0].trim().parse::<u32>() else {
            continue;
        };
        if pid == 0 {
            continue;
        }
        raw_processes.push(RawProcessListItem {
            pid,
            ppid: fields[1].trim().parse::<u32>().unwrap_or(0),
            user: fields[2].trim().to_string(),
            mem_bytes: fields[3].trim().parse::<u64>().unwrap_or(0) * 1024,
            cpu: fields[4].trim().parse::<f64>().unwrap_or(0.0),
            start_time: fields[5].trim().parse::<u64>().unwrap_or(0),
            name: fields[6].trim().to_string(),
            command: fields[7].trim().to_string(),
        });
    }

    let excluded = collector_process_ids(
        raw_processes.iter().map(|process| {
            (
                process.pid,
                process.ppid,
                is_collector_process(&process.name, &process.command),
            )
        }),
        collector_roots,
    );
    Ok(raw_processes
        .into_iter()
        .filter(|process| !excluded.contains(&process.pid))
        .map(|process| ProcessListItem {
            pid: process.pid,
            user: process.user,
            mem_bytes: process.mem_bytes,
            cpu: process.cpu,
            start_time: process.start_time,
            name: process.name,
            executable: executables
                .remove(&(process.pid, process.start_time))
                .unwrap_or_default(),
            command: process.command,
        })
        .collect())
}

/// 将十六进制文本还原为原始字节
fn decode_hex(value: &str) -> Result<Vec<u8>> {
    let value = value.trim();
    if !value.len().is_multiple_of(2) {
        return Err(anyhow!("远端进程详情编码长度无效"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let pair = std::str::from_utf8(pair).map_err(|_| anyhow!("远端进程详情编码无效"))?;
        bytes.push(u8::from_str_radix(pair, 16).map_err(|_| anyhow!("远端进程详情编码无效"))?);
    }
    Ok(bytes)
}

/// 将十六进制字段解码为 UTF-8 文本，无法解码的字节使用替换字符保留位置
fn decode_text(value: &str) -> Result<String> {
    Ok(String::from_utf8_lossy(&decode_hex(value)?).into_owned())
}

/// 将 NUL 分隔的十六进制字段解码为字符串列表
fn decode_nul_strings(value: &str) -> Result<Vec<String>> {
    Ok(decode_hex(value)?
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .collect())
}

/// 解析单个进程详情脚本的输出
fn parse_process_detail(raw: &str, pid: u32) -> Result<ProcessDetail> {
    if raw.lines().any(|line| line.trim() == DETAIL_GONE_MARKER) {
        return Err(anyhow!("进程已结束或 PID 已被其他进程复用"));
    }
    if !raw.lines().any(|line| line.trim() == DETAIL_DONE_MARKER) {
        return Err(anyhow!("远端进程详情返回不完整"));
    }

    let fields: HashMap<&str, &str> = raw
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    let trim_link_newline = |value: String| value.trim_end_matches(['\r', '\n']).to_string();
    let command = decode_nul_strings(fields.get("COMMAND").copied().unwrap_or_default())?.join(" ");
    let mut environment =
        decode_nul_strings(fields.get("ENVIRONMENT").copied().unwrap_or_default())?
            .into_iter()
            .map(|entry| {
                let (name, value) = entry.split_once('=').unwrap_or((&entry, ""));
                ProcessEnvironmentVariable {
                    name: name.to_string(),
                    value: value.to_string(),
                }
            })
            .collect::<Vec<_>>();
    environment.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.value.cmp(&b.value)));

    Ok(ProcessDetail {
        pid,
        name: decode_text(fields.get("NAME").copied().unwrap_or_default())?
            .trim_end_matches(['\r', '\n'])
            .to_string(),
        command,
        executable: trim_link_newline(decode_text(
            fields.get("EXECUTABLE").copied().unwrap_or_default(),
        )?),
        working_directory: trim_link_newline(decode_text(
            fields.get("WORKING_DIRECTORY").copied().unwrap_or_default(),
        )?),
        environment,
    })
}

/// 将 PID 和启动时钟值填入远端脚本模板
fn fill_process_script(template: &str, pid: u32, start_time: u64) -> String {
    template
        .replace("__PID__", &pid.to_string())
        .replace("__START_TIME__", &start_time.to_string())
}

/// 校验前端提交的进程身份字段
fn validate_process_identity(pid: u32, start_time: u64) -> Result<()> {
    if pid == 0 || start_time == 0 {
        return Err(anyhow!("无法确认目标进程身份，请刷新进程列表后重试"));
    }
    Ok(())
}

/// 查询远端完整进程列表
pub async fn list(manager: &SessionManager, session_id: &str) -> Result<Vec<ProcessListItem>> {
    manager.monitor_state(session_id)?;
    let raw = timeout(
        PROCESS_TIMEOUT,
        manager.exec(session_id, PROCESS_LIST_SCRIPT),
    )
    .await
    .map_err(|_| anyhow!("进程列表查询超时"))??;
    parse_process_list(&raw)
}

/// 查询远端单个进程的详细信息
pub async fn detail(
    manager: &SessionManager,
    session_id: &str,
    pid: u32,
    start_time: u64,
) -> Result<ProcessDetail> {
    manager.monitor_state(session_id)?;
    validate_process_identity(pid, start_time)?;
    let command = fill_process_script(PROCESS_DETAIL_SCRIPT, pid, start_time);
    let raw = timeout(PROCESS_TIMEOUT, manager.exec(session_id, &command))
        .await
        .map_err(|_| anyhow!("进程详情查询超时"))??;
    parse_process_detail(&raw, pid)
}

/// 向远端目标进程发送 SIGTERM
pub async fn terminate(
    manager: &SessionManager,
    session_id: &str,
    pid: u32,
    start_time: u64,
) -> Result<()> {
    manager.monitor_state(session_id)?;
    validate_process_identity(pid, start_time)?;
    let command = fill_process_script(PROCESS_TERMINATE_SCRIPT, pid, start_time);
    let raw = timeout(PROCESS_TIMEOUT, manager.exec(session_id, &command))
        .await
        .map_err(|_| anyhow!("终止进程操作超时"))??;
    if raw.contains(TERMINATE_DONE_MARKER) {
        return Ok(());
    }
    if raw.contains(TERMINATE_GONE_MARKER) {
        return Err(anyhow!("进程已结束或 PID 已被其他进程复用"));
    }
    if raw.contains(TERMINATE_DENIED_MARKER) {
        return Err(anyhow!("远端系统拒绝终止该进程，请检查当前用户权限"));
    }
    Err(anyhow!("远端终止进程操作返回不完整"))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::*;

    /// 将测试字节编码为协议使用的十六进制文本
    fn encode_hex(bytes: &[u8]) -> String {
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(&mut output, "{byte:02x}").unwrap();
        }
        output
    }

    /// 列表解析应保留完整命令行、批量路径结果并将 RSS 从 KiB 转为字节
    #[test]
    fn parses_complete_process_list() {
        let raw = format!(
            "{LIST_META_PREFIX}\x1f900\n 42\x1f1\x1falice\x1f 2048\x1f12.5\x1f9876\x1fpython\x1fpython main.py\x1f--flag\n{LIST_EXE_MARKER}\n/proc/42\x1f9876\x1f/usr/bin/python3\n{LIST_DONE_MARKER}\n"
        );

        let processes = parse_process_list(&raw).unwrap();

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].pid, 42);
        assert_eq!(processes[0].user, "alice");
        assert_eq!(processes[0].mem_bytes, 2 * 1024 * 1024);
        assert_eq!(processes[0].cpu, 12.5);
        assert_eq!(processes[0].start_time, 9876);
        assert_eq!(processes[0].name, "python");
        assert_eq!(processes[0].executable, "/usr/bin/python3");
        assert_eq!(processes[0].command, "python main.py\x1f--flag");
    }

    /// 列表解析应递归排除当前及并发运行的 ZTShell 采集器进程树
    #[test]
    fn filters_collector_process_trees() {
        let raw = format!(
            "{LIST_META_PREFIX}\x1f100\n\
             101\x1f100\x1froot\x1f10\x1f80\x1f1\x1fps\x1fps -e\n\
             100\x1f99\x1froot\x1f20\x1f40\x1f2\x1fztshell-proc\x1fsh -c : __ZT_PROCESS_COLLECTOR__\n\
             201\x1f200\x1froot\x1f10\x1f60\x1f3\x1fawk\x1fawk collector\n\
             200\x1f1\x1froot\x1f20\x1f30\x1f4\x1fztshell-mon\x1fsh -c : __ZT_MONITOR_COLLECTOR__\n\
             300\x1f1\x1falice\x1f30\x1f5\x1f5\x1fps\x1fps legitimate-job\n\
             400\x1f1\x1falice\x1f30\x1f4\x1f6\x1fztshell-mon\x1f/usr/bin/ztshell-mon --serve\n\
             401\x1f400\x1falice\x1f30\x1f3\x1f7\x1fworker\x1fworker --serve\n\
             {LIST_EXE_MARKER}\n/proc/300\x1f5\x1f/usr/bin/ps\n{LIST_DONE_MARKER}\n"
        );

        let processes = parse_process_list(&raw).unwrap();

        assert_eq!(processes.len(), 3);
        assert_eq!(processes[0].pid, 300);
        assert_eq!(processes[0].executable, "/usr/bin/ps");
        assert_eq!(processes[1].pid, 400);
        assert_eq!(processes[2].pid, 401);
    }

    /// 可执行路径的启动时钟不匹配时不得拼接到旧 PID 快照
    #[test]
    fn rejects_executable_from_reused_pid() {
        let raw = format!(
            "42\x1f1\x1falice\x1f2048\x1f12.5\x1f9876\x1fpython\x1fpython old.py\n\
             {LIST_EXE_MARKER}\n/proc/42\x1f9877\x1f/usr/bin/new-process\n{LIST_DONE_MARKER}\n"
        );

        let processes = parse_process_list(&raw).unwrap();

        assert_eq!(processes.len(), 1);
        assert!(processes[0].executable.is_empty());
    }

    /// PID 1 不得成为过滤根，避免异常执行环境隐藏全部系统进程
    #[test]
    fn never_filters_from_init_process() {
        let excluded = collector_process_ids([(2, 1, false)], [1]);

        assert!(excluded.is_empty());
    }

    /// 列表解析应跳过竞态期间产生的不完整行
    #[test]
    fn skips_incomplete_process_rows() {
        let raw = format!("bad row\n{LIST_DONE_MARKER}\n");

        assert!(parse_process_list(&raw).unwrap().is_empty());
    }

    /// 列表脚本不应使用 Ubuntu 24.04 中 procps-ng 不支持的分隔符选项
    #[test]
    fn process_list_script_uses_supported_procps_options() {
        assert!(PROCESS_LIST_SCRIPT
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .all(|line| !line.contains("--delimiter")));
        assert!(PROCESS_LIST_SCRIPT.contains("-o args="));
        assert!(PROCESS_LIST_SCRIPT.contains("find /proc -mindepth 2"));
        assert!(PROCESS_LIST_SCRIPT.contains("__ZT_PROCESS_COLLECTOR__"));
    }

    /// 缺少结束标记时应拒绝使用不完整列表
    #[test]
    fn rejects_incomplete_process_list() {
        let error = parse_process_list("1\x1froot").unwrap_err();

        assert_eq!(error.to_string(), "远端进程列表返回不完整");
    }

    /// 详情解析应按 NUL 边界还原命令参数和环境变量
    #[test]
    fn parses_process_detail() {
        let raw = format!(
            "NAME={}\nCOMMAND={}\nEXECUTABLE={}\nWORKING_DIRECTORY={}\nENVIRONMENT={}\n{DETAIL_DONE_MARKER}\n",
            encode_hex(b"python\n"),
            encode_hex(b"python\0main.py\0--port=8080\0"),
            encode_hex(b"/usr/bin/python3\n"),
            encode_hex(b"/srv/app\n"),
            encode_hex(b"HOME=/root\0EMPTY=\0LANG=C.UTF-8\0"),
        );

        let detail = parse_process_detail(&raw, 42).unwrap();

        assert_eq!(detail.pid, 42);
        assert_eq!(detail.name, "python");
        assert_eq!(detail.command, "python main.py --port=8080");
        assert_eq!(detail.executable, "/usr/bin/python3");
        assert_eq!(detail.working_directory, "/srv/app");
        assert_eq!(
            detail.environment,
            vec![
                ProcessEnvironmentVariable {
                    name: "EMPTY".to_string(),
                    value: String::new(),
                },
                ProcessEnvironmentVariable {
                    name: "HOME".to_string(),
                    value: "/root".to_string(),
                },
                ProcessEnvironmentVariable {
                    name: "LANG".to_string(),
                    value: "C.UTF-8".to_string(),
                },
            ]
        );
    }

    /// 进程身份中的零值应被拒绝，避免向错误目标发送信号
    #[test]
    fn rejects_unknown_process_identity() {
        assert!(validate_process_identity(0, 1).is_err());
        assert!(validate_process_identity(1, 0).is_err());
    }
}
