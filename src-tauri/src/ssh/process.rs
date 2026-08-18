//! 远端 Linux 进程列表、详情与操作

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Serialize;
use tokio::time::timeout;

use super::manager::SessionManager;

/// 单次进程查询或操作允许的最长时间
const PROCESS_TIMEOUT: Duration = Duration::from_secs(15);
/// 进程列表脚本执行失败标记
const LIST_ERROR_MARKER: &str = "__ZT_PROCESS_LIST_ERROR__";
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
# procps-ng 4.x 不支持 --delimiter，命令行必须放在最后一列以保留其中的空格。
raw="$(ps -ww -e --sort=-pcpu \
  -o pid= -o user:64= -o rss= -o pcpu= -o args= 2>/dev/null)" || {
  printf '__ZT_PROCESS_LIST_ERROR__\n'
  exit
}

printf '%s\n' "$raw" |
while read -r pid user rss cpu command; do
  [ -n "$pid" ] || continue

  start_time=0
  if IFS= read -r stat_line 2>/dev/null < "/proc/$pid/stat"; then
    stat_rest=${stat_line##*) }
    set -- $stat_rest
    start_time=${20:-0}
  fi
  name=""
  IFS= read -r name 2>/dev/null < "/proc/$pid/comm" || true
  executable="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"

  printf '%s\037%s\037%s\037%s\037%s\037%s\037%s\037%s\n' \
    "$pid" "$user" "$rss" "$cpu" "$start_time" "$name" "$executable" "$command"
done
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

/// 将列表脚本输出解析为结构化进程条目
fn parse_process_list(raw: &str) -> Result<Vec<ProcessListItem>> {
    if raw.lines().any(|line| line.trim() == LIST_ERROR_MARKER) {
        return Err(anyhow!("远端系统无法读取进程列表"));
    }
    if !raw.lines().any(|line| line.trim() == LIST_DONE_MARKER) {
        return Err(anyhow!("远端进程列表返回不完整"));
    }

    let mut processes = Vec::new();
    for line in raw.lines() {
        if line.trim() == LIST_DONE_MARKER {
            break;
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
        processes.push(ProcessListItem {
            pid,
            user: fields[1].trim().to_string(),
            mem_bytes: fields[2].trim().parse::<u64>().unwrap_or(0) * 1024,
            cpu: fields[3].trim().parse::<f64>().unwrap_or(0.0),
            start_time: fields[4].trim().parse::<u64>().unwrap_or(0),
            name: fields[5].trim().to_string(),
            executable: fields[6].trim().to_string(),
            command: fields[7].trim().to_string(),
        });
    }
    Ok(processes)
}

/// 将十六进制文本还原为原始字节
fn decode_hex(value: &str) -> Result<Vec<u8>> {
    let value = value.trim();
    if !value.len().is_multiple_of(2) {
        return Err(anyhow!("远端进程详情编码长度无效"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
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

    /// 列表解析应保留完整命令行并将 RSS 从 KiB 转为字节
    #[test]
    fn parses_complete_process_list() {
        let raw = format!(
            " 42\x1falice\x1f 2048\x1f12.5\x1f9876\x1fpython\x1f/usr/bin/python3\x1fpython main.py\x1f--flag\n{LIST_DONE_MARKER}\n"
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
