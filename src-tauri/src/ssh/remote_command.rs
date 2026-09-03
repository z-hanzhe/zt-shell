//! 可中断远端命令协议：进程组隔离、取消标记与控制脚本生成。

use uuid::Uuid;

/// 单次可中断远端命令的控制计划。
pub(super) struct CancellableCommandPlan {
    wrapped_command: String,
    cancel_command: String,
    failure_marker: String,
}

impl CancellableCommandPlan {
    /// 为远端命令创建隔离的控制目录和随机失败标记。
    pub(super) fn new(command: &str) -> Self {
        let operation_id = Uuid::new_v4().simple().to_string();
        let operation_dir = format!("/tmp/ztshell-operation-{}", operation_id);
        let failure_marker = format!("__ZTCONTROLFAIL_{}__", operation_id);
        Self::with_identity(command, &operation_dir, &failure_marker)
    }

    /// 返回提交给主 SSH exec 通道的包装命令。
    pub(super) fn wrapped_command(&self) -> &str {
        &self.wrapped_command
    }

    /// 返回通过独立 SSH exec 通道投递取消标记的命令。
    pub(super) fn cancel_command(&self) -> &str {
        &self.cancel_command
    }

    /// 判断远端输出是否包含本次操作独有的控制环境失败标记。
    pub(super) fn is_control_failure(&self, output: &str) -> bool {
        output.contains(&self.failure_marker)
    }

    /// 使用指定标识构造控制计划，供稳定测试覆盖边界场景。
    fn with_identity(command: &str, operation_dir: &str, failure_marker: &str) -> Self {
        Self {
            wrapped_command: build_cancellable_command(command, operation_dir, failure_marker),
            cancel_command: build_remote_cancel_command(operation_dir),
            failure_marker: failure_marker.to_string(),
        }
    }
}

/// 转义传给远端 POSIX shell 的单个参数。
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 包装远端命令，由 SSH 进程组组长等待 payload 并处理取消。
fn build_cancellable_command(command: &str, operation_dir: &str, failure_marker: &str) -> String {
    let cancel_dir = format!("{}/cancel", operation_dir);
    format!(
        concat!(
            "umask 077; ZT_OPERATION_DIR={}; ZT_CANCEL_DIR={}; ",
            "ZT_COMMAND={}; ZT_WRAPPER_PID=$$; ZT_CANCELLED=0; ",
            "if ! mkdir -m 700 \"$ZT_OPERATION_DIR\" 2>/dev/null; then ",
            "printf '%s' {}; exit 125; fi; ",
            "ZT_PGID=$(ps -o pgid= -p $$ 2>/dev/null | tr -d ' '); ",
            "case \"$ZT_PGID\" in ''|*[!0-9]*) ",
            "rm -rf \"$ZT_OPERATION_DIR\"; printf '%s' {}; exit 125;; esac; ",
            "if [ \"$ZT_PGID\" != \"$$\" ]; then ",
            "rm -rf \"$ZT_OPERATION_DIR\"; printf '%s' {}; exit 125; fi; ",
            "trap 'ZT_CANCELLED=1' TERM HUP INT USR1; ",
            "if [ -d \"$ZT_CANCEL_DIR\" ]; then ",
            "rm -rf \"$ZT_OPERATION_DIR\"; exit 143; fi; ",
            "sh -c \"$ZT_COMMAND\" & ZT_PAYLOAD_PID=$!; ",
            "(while kill -0 \"$ZT_WRAPPER_PID\" 2>/dev/null; do ",
            "if [ -d \"$ZT_CANCEL_DIR\" ]; then ",
            "kill -USR1 \"$ZT_WRAPPER_PID\" 2>/dev/null; exit 0; fi; ",
            "sleep 0.1; done) & ZT_WATCHER_PID=$!; ",
            "if [ \"$ZT_CANCELLED\" -eq 0 ] && [ ! -d \"$ZT_CANCEL_DIR\" ]; then ",
            "wait \"$ZT_PAYLOAD_PID\"; ZT_STATUS=$?; else ZT_STATUS=143; fi; ",
            "if [ \"$ZT_CANCELLED\" -eq 1 ] || [ -d \"$ZT_CANCEL_DIR\" ]; then ",
            "trap '' TERM HUP INT USR1; kill -TERM \"-$ZT_PGID\" 2>/dev/null; ",
            "rm -rf \"$ZT_OPERATION_DIR\"; sleep 0.5; ",
            "kill -KILL \"-$ZT_PGID\" 2>/dev/null; exit 143; fi; ",
            "kill -TERM \"$ZT_WATCHER_PID\" 2>/dev/null; ",
            "wait \"$ZT_WATCHER_PID\" 2>/dev/null; trap - TERM HUP INT USR1; ",
            "rm -rf \"$ZT_OPERATION_DIR\"; exit \"$ZT_STATUS\""
        ),
        shell_quote(operation_dir),
        shell_quote(&cancel_dir),
        shell_quote(command),
        shell_quote(failure_marker),
        shell_quote(failure_marker),
        shell_quote(failure_marker)
    )
}

/// 构建取消命令，只在包装器创建的私有目录中投递原子标记。
fn build_remote_cancel_command(operation_dir: &str) -> String {
    let cancel_dir = format!("{}/cancel", operation_dir);
    format!(
        concat!(
            "ZT_OPERATION_DIR={}; ZT_CANCEL_DIR={}; ZT_TRY=0; ",
            "while [ \"$ZT_TRY\" -lt 20 ] && [ ! -d \"$ZT_OPERATION_DIR\" ]; do ",
            "ZT_TRY=$((ZT_TRY + 1)); sleep 0.1; done; ",
            "if [ -d \"$ZT_OPERATION_DIR\" ]; then ",
            "mkdir -m 700 \"$ZT_CANCEL_DIR\" 2>/dev/null || ",
            "[ -d \"$ZT_CANCEL_DIR\" ]; fi"
        ),
        shell_quote(operation_dir),
        shell_quote(&cancel_dir)
    )
}

#[cfg(test)]
mod tests {
    use std::process::Command as StdCommand;
    #[cfg(target_os = "linux")]
    use std::time::Duration;

    #[cfg(target_os = "linux")]
    use tokio::time::timeout;
    #[cfg(target_os = "linux")]
    use uuid::Uuid;

    use super::*;

    /// 控制计划应使用随机标记并生成符合 POSIX shell 语法的命令。
    #[test]
    fn builds_isolated_control_plan() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");

        let plan = CancellableCommandPlan::with_identity(
            "printf '%s' test",
            "/tmp/ztshell-operation-test",
            "__ZTCONTROLFAIL_TEST__",
        );
        assert!(plan.wrapped_command().contains("umask 077"));
        assert!(plan
            .wrapped_command()
            .contains("[ \"$ZT_PGID\" != \"$$\" ]"));
        assert!(plan.wrapped_command().contains("wait \"$ZT_PAYLOAD_PID\""));
        assert!(plan.wrapped_command().contains("kill -KILL \"-$ZT_PGID\""));
        assert!(plan
            .cancel_command()
            .contains("mkdir -m 700 \"$ZT_CANCEL_DIR\""));
        assert!(!plan.cancel_command().contains("kill -"));
        assert!(!plan.is_control_failure("__ZTCONTROLFAIL__"));
        assert!(plan.is_control_failure("prefix__ZTCONTROLFAIL_TEST__suffix"));

        for command in [plan.wrapped_command(), plan.cancel_command()] {
            if let Ok(status) = StdCommand::new("sh").args(["-n", "-c", command]).status() {
                assert!(status.success(), "远端控制命令应符合 POSIX shell 语法");
            }
        }
    }

    /// Linux 上投递取消标记后应终止包装器所在的完整进程组。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancels_wrapped_process_group() {
        let operation_id = Uuid::new_v4().simple().to_string();
        let operation_dir = format!("/tmp/ztshell-test-operation-{}", operation_id);
        let process_group_file = format!("/tmp/ztshell-test-pgid-{}", operation_id);
        let command = format!(
            "ps -o pgid= -p $$ | tr -d ' ' > {}; sleep 30",
            shell_quote(&process_group_file)
        );
        let plan = CancellableCommandPlan::with_identity(
            &command,
            &operation_dir,
            "__ZTCONTROLFAIL_TEST__",
        );
        let mut process = tokio::process::Command::new("setsid")
            .arg("sh")
            .arg("-c")
            .arg(plan.wrapped_command())
            .spawn()
            .expect("应能启动可中断测试命令");

        for _ in 0..100 {
            if tokio::fs::metadata(&process_group_file).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let process_group = tokio::fs::read_to_string(&process_group_file)
            .await
            .expect("测试命令应发布进程组")
            .trim()
            .to_string();
        assert!(process_group
            .chars()
            .all(|character| character.is_ascii_digit()));

        let cancel_status = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(plan.cancel_command())
            .status()
            .await
            .expect("应能执行取消命令");
        assert!(cancel_status.success());
        timeout(Duration::from_secs(5), process.wait())
            .await
            .expect("包装命令应在取消后及时退出")
            .expect("应能取得包装命令退出状态");

        let process_target = format!("-{}", process_group);
        let still_running = tokio::process::Command::new("kill")
            .args(["-0", "--", &process_target])
            .status()
            .await
            .expect("应能检查测试进程状态")
            .success();
        assert!(!still_running, "取消后不应遗留远端子进程");
        assert!(tokio::fs::metadata(&operation_dir).await.is_err());

        let _ = tokio::fs::remove_file(process_group_file).await;
        let _ = tokio::fs::remove_dir_all(operation_dir).await;
    }

    /// payload 删除控制目录后仍应由 wait 正常回收，不能依赖完成标记。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn completes_when_payload_removes_control_directory() {
        let operation_id = Uuid::new_v4().simple().to_string();
        let operation_dir = format!("/tmp/ztshell-test-operation-{}", operation_id);
        let command = format!(
            "rm -rf {}; printf payload-finished",
            shell_quote(&operation_dir)
        );
        let plan = CancellableCommandPlan::with_identity(
            &command,
            &operation_dir,
            "__ZTCONTROLFAIL_TEST__",
        );
        let output = timeout(
            Duration::from_secs(5),
            tokio::process::Command::new("setsid")
                .arg("sh")
                .arg("-c")
                .arg(plan.wrapped_command())
                .output(),
        )
        .await
        .expect("删除控制目录后包装器仍应及时退出")
        .expect("应能执行控制目录回归测试");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "payload-finished");

        let _ = tokio::fs::remove_dir_all(operation_dir).await;
    }
}
