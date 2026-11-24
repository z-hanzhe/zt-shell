//! 普通文件管理模式的远端批量删除和权限修改。

use anyhow::{anyhow, Result};
use tokio::sync::watch;

use super::manager::SessionManager;
use super::remote_command::shell_quote;
use super::session::{ExecOutput, ExecRequestRejected};

// 包装和转义后也需留在远端 shell 的单参数长度限制内。
const MAX_BATCH_BYTES: usize = 24 * 1024;
const BATCH_PREFIX: &str = "LC_ALL=C; export LC_ALL\n";
const STOP_ON_FAILURE: &str = " || exit $?\n";
const SFTP_ONLY_MESSAGE: &str = "This service allows sftp connections only.";

/// 按传入顺序执行一批文件操作，失败时停止。仅在首批明确未启动时返回 `false`，允许调用方使用 SFTP。
pub(super) async fn run_batch(
    manager: &SessionManager,
    session_id: &str,
    commands: Vec<String>,
    failure_message: &str,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<bool> {
    let mut commands = commands.into_iter().peekable();
    let mut started = false;
    while let Some(first) = commands.next() {
        let mut batch = String::from(BATCH_PREFIX);
        batch.push_str(&first);
        batch.push_str(STOP_ON_FAILURE);
        while let Some(next) = commands.peek() {
            if batch.len() + next.len() + STOP_ON_FAILURE.len() > MAX_BATCH_BYTES {
                break;
            }
            batch.push_str(next);
            batch.push_str(STOP_ON_FAILURE);
            commands.next();
        }
        if batch.len() > MAX_BATCH_BYTES {
            return Err(anyhow!("远端文件路径过长"));
        }

        let output = match manager
            .exec_cancellable_output(session_id, &batch, cancellation)
            .await
        {
            Err(error) if !started && error.is::<ExecRequestRejected>() => return Ok(false),
            result => result?,
        };
        // OpenSSH 的强制 SFTP 模式会接收 exec 请求，但直接返回提示文本。
        if !started
            && output.exit_status == Some(1)
            && ((output.stderr.is_empty() && output.stdout.trim() == SFTP_ONLY_MESSAGE)
                || (output.stdout.is_empty() && output.stderr.trim() == SFTP_ONLY_MESSAGE))
        {
            return Ok(false);
        }
        if output.exit_status != Some(0) {
            return Err(batch_error(failure_message, &output));
        }
        started = true;
    }
    Ok(true)
}

/// 根据远端退出状态及标准错误生成失败提示。
fn batch_error(message: &str, output: &ExecOutput) -> anyhow::Error {
    let detail = output.stderr.trim();
    if detail.lines().any(|line| {
        line.ends_with(": Permission denied") || line.ends_with(": Operation not permitted")
    }) {
        anyhow!("{}：权限不足。{}", message, detail)
    } else if !detail.is_empty() {
        anyhow!("{}：{}", message, detail)
    } else if let Some(status) = output.exit_status {
        anyhow!("{}：远端命令退出状态为 {}", message, status)
    } else {
        anyhow!("{}：未收到远端命令的退出状态", message)
    }
}

/// 构建单个删除命令，只有确认删除的目录使用递归参数。
pub(super) fn remove_command(path: &str, is_dir: bool) -> String {
    let path = shell_quote(path.trim_end_matches('/'));
    if is_dir {
        format!("rm -rf -- {path}")
    } else {
        format!("rm -- {path}")
    }
}

/// 将九位权限转换为显式清除特殊权限位的 chmod 参数。
pub(super) fn symbolic_mode(mode: u32) -> String {
    let mut argument = String::from("u-s,g-s,a-t");
    for (who, shift) in [('u', 6), ('g', 3), ('o', 0)] {
        argument.push_str(&format!(",{who}="));
        for (bit, permission) in [(4, 'r'), (2, 'w'), (1, 'x')] {
            if (mode >> shift) & bit != 0 {
                argument.push(permission);
            }
        }
    }
    argument
}

/// 构建单个目标的改权命令，递归时按范围选择条目并先处理子项。
pub(super) fn permissions_command(path: &str, mode: &str, recursive: bool, scope: &str) -> String {
    let path = shell_quote(path.trim_end_matches('/'));
    let action = if recursive {
        let filter = match scope {
            "files" => "-type f",
            "directories" => "-type d",
            _ => "! -type l",
        };
        // find 默认不跟随链接，-depth 保证子项排在父目录之前。
        format!("find \"$ZT_PATH\" -depth {filter} -exec chmod {mode} {{}} +")
    } else {
        format!("chmod {mode} \"$ZT_PATH\"")
    };
    format!(
        "ZT_PATH={path}; if [ -L \"$ZT_PATH\" ]; then \
         printf '%s\\n' '符号链接不支持直接修改权限' >&2; (exit 1); else {action}; fi"
    )
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::process::Command;

    use super::*;

    /// 远端脚本在临时目录中运行，仅在 Windows 未安装 POSIX shell 时跳过。
    fn run_shell_case(script: &str) {
        let output = match Command::new("sh").args(["-eu", "-c", script]).output() {
            Err(error) if cfg!(windows) && error.kind() == ErrorKind::NotFound => return,
            result => result.expect("应能启动测试 shell"),
        };
        assert!(
            output.status.success(),
            "测试命令应成功：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// 仅递归删除确认过的目录，带单引号和空格的路径仍应保留原义。
    #[test]
    fn removal_preserves_target_type_and_literal_path() {
        let file = remove_command("./file's name", false);
        let dir = remove_command("./directory/", true);
        let replaced_file = remove_command("./directory", false);
        run_shell_case(&format!(
            "tmp=$(mktemp -d); trap 'rm -rf -- \"$tmp\"' EXIT; cd \"$tmp\"\n\
             touch \"file's name\"; mkdir directory; touch directory/child\n\
             {file}; test ! -e \"file's name\"\n\
             if {replaced_file}; then exit 1; fi; test -f directory/child\n\
             {dir}; test ! -e directory"
        ));
    }

    /// 递归改权应分别筛选条目范围，清除特殊位并跳过目录内的链接。
    #[test]
    #[cfg(unix)]
    fn permissions_apply_scope_without_following_links() {
        let mode = symbolic_mode(0o750);
        let files = permissions_command("./dir's name", &mode, true, "files");
        let dirs = permissions_command("./dir's name", &mode, true, "directories");
        let all = permissions_command("./dir's name", &mode, true, "all");
        let link = permissions_command("./dir's name/link/", &mode, false, "all");
        run_shell_case(&format!(
            "tmp=$(mktemp -d); trap 'rm -rf -- \"$tmp\"' EXIT; cd \"$tmp\"\n\
             mkdir -p \"dir's name/sub\"; touch \"dir's name/sub/file\" outside\n\
             chmod 0700 \"dir's name\"; chmod 3700 \"dir's name/sub\"\n\
             chmod 0600 outside; ln -s ../outside \"dir's name/link\"\n\
             {files}; test -n \"$(find \"dir's name/sub/file\" -perm 0750)\"\n\
             test -n \"$(find \"dir's name\" -prune -perm 0700)\"\n\
             chmod 0600 \"dir's name/sub/file\"\n\
             {dirs}; test -n \"$(find \"dir's name/sub\" -prune -perm 0750)\"\n\
             test -n \"$(find \"dir's name/sub/file\" -perm 0600)\"\n\
             {all}; test -n \"$(find \"dir's name/sub/file\" -perm 0750)\"\n\
             if {link}; then exit 1; fi; test -n \"$(find outside -perm 0600)\""
        ));
    }

    /// 远端报错应保留原始原因，并区分权限不足与其他执行错误。
    #[test]
    fn reports_permission_denied_without_masking_other_errors() {
        let mut output = ExecOutput {
            stdout: String::new(),
            stderr: "chmod: /file: Permission denied".into(),
            exit_status: Some(1),
        };
        assert!(batch_error("修改权限失败", &output)
            .to_string()
            .contains("权限不足"));
        output.stderr = "chmod: /file: Read-only file system".into();
        let error = batch_error("修改权限失败", &output).to_string();
        assert!(error.contains(&output.stderr));
        assert!(!error.contains("权限不足"));
    }
}
