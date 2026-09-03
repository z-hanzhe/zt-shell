//! 打包下载运行时状态：打包参数与已验证归档的原子所有权。

use std::sync::Mutex;

/// 打包下载的不可变参数。
#[derive(Debug, Clone)]
pub(super) struct ArchiveJob {
    /// 打包目标所在的远端目录。
    pub(super) remote_dir: String,
    /// 参与打包的条目名称列表。
    pub(super) names: Vec<String>,
}

/// 已完成且可供下载的远端临时归档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadyArchive {
    /// 远端临时归档路径。
    pub(super) path: String,
    /// 归档完成时记录的字节数。
    pub(super) size: u64,
}

/// 单个打包下载任务的归档状态。
pub(super) struct ArchiveRuntime {
    job: ArchiveJob,
    ready: Mutex<Option<ReadyArchive>>,
}

impl ArchiveRuntime {
    /// 创建尚未生成远端归档的打包任务状态。
    pub(super) fn new(remote_dir: String, names: Vec<String>) -> Self {
        Self {
            job: ArchiveJob { remote_dir, names },
            ready: Mutex::new(None),
        }
    }

    /// 返回打包参数副本，避免跨异步等待持有状态锁。
    pub(super) fn job(&self) -> ArchiveJob {
        self.job.clone()
    }

    /// 返回当前已验证归档副本。
    pub(super) fn ready(&self) -> Option<ReadyArchive> {
        self.ready.lock().unwrap().clone()
    }

    /// 原子发布已验证的远端归档。
    pub(super) fn publish(&self, path: String, size: u64) {
        *self.ready.lock().unwrap() = Some(ReadyArchive { path, size });
    }

    /// 取走当前归档所有权，调用方负责远端清理。
    pub(super) fn take_ready(&self) -> Option<ReadyArchive> {
        self.ready.lock().unwrap().take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 归档路径与大小应作为一个整体发布和取走。
    #[test]
    fn owns_ready_archive_as_single_value() {
        let archive = ArchiveRuntime::new("/remote".to_string(), vec!["file".to_string()]);
        assert!(archive.ready().is_none());

        archive.publish("/tmp/archive.tar.gz".to_string(), 128);
        assert_eq!(
            archive.ready(),
            Some(ReadyArchive {
                path: "/tmp/archive.tar.gz".to_string(),
                size: 128,
            })
        );
        assert!(archive.take_ready().is_some());
        assert!(archive.ready().is_none());
    }
}
