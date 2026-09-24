//! 传输任务管理器：上传/下载任务队列、断点续传、暂停继续、打包下载与进度事件推送
//!
//! 设计要点：
//! - 任务组织为树：文件夹任务是聚合节点（进度/状态由子任务汇总），文件任务是实际执行单元
//! - 并发由全局信号量限制，同一时刻最多 3 个文件在传输，其余任务排队等待
//! - 断点续传：任务曾经运行过（started_once）时按已落盘字节 seek 续传，首次运行覆盖写
//! - 网络波动自动重试 3 次，超过后标记失败，可通过"重试失败的作业"手动续传
//! - 进度由后台定时循环节流推送：结构变化推 transfer://changed 全量，动态变化推 transfer://progress 增量

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Error, Result};
use dashmap::{DashMap, DashSet};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use tokio::time::{error::Elapsed, timeout};

use super::manager::SessionManager;
use super::remote_command::shell_quote;
use super::sftp::format_sftp_error;

mod archive;
mod control;

use archive::ArchiveRuntime;
use control::{
    status_str, ResumeAction, TaskControl, ST_CANCELLED, ST_COMPLETED, ST_FAILED, ST_PACKING,
    ST_PAUSED, ST_PENDING, ST_RUNNING,
};

/// 会话断开时保留任务使用的失败原因
const SESSION_INTERRUPTED_MESSAGE: &str = "SSH 会话已断开，重连后可重试任务";

/// 同时传输的文件数上限
const MAX_CONCURRENT: usize = 3;
/// 单块读写缓冲大小（64KB，兼容 OpenSSH 单包上限）
const CHUNK_SIZE: usize = 64 * 1024;
/// 传输失败自动重试次数
const MAX_ATTEMPTS: u32 = 3;
/// 自动重试间隔（毫秒）
const RETRY_DELAY_MS: u64 = 2000;
/// 文件总数确认阈值（本次文件数与会话内未完成任务之和），超过时提示打包压缩
const CONFIRM_THRESHOLD: u64 = 50;
/// 文件总数上限（本次文件数与会话内未完成任务之和），超过时直接拒绝创建
const MAX_TOTAL_FILES: u64 = 100;
/// 进度推送节流间隔（毫秒）
const TICK_MS: u64 = 1000;
/// 清理远端临时文件的最长等待时间
const REMOTE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
/// 校验远端打包文件的最长等待时间
const ARCHIVE_METADATA_TIMEOUT: Duration = Duration::from_secs(10);
/// 单次 SFTP 下载请求的最长等待时间
const SFTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 关闭 SFTP 文件句柄的最长等待时间
const SFTP_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

/// 传输方向
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransferKind {
    /// 上传
    Upload,
    /// 下载
    Download,
}

impl TransferKind {
    /// 转为前端展示用字符串
    fn as_str(&self) -> &'static str {
        match self {
            TransferKind::Upload => "upload",
            TransferKind::Download => "download",
        }
    }
}

/// 单个传输任务的运行时状态
pub struct TaskState {
    /// 任务唯一标识
    pub id: String,
    /// 父任务标识（文件夹子项持有）
    pub parent_id: Option<String>,
    /// 所属会话标识
    pub session_id: String,
    /// 传输方向
    pub kind: TransferKind,
    /// 是否为目录节点
    pub is_dir: bool,
    /// 展示名称
    pub name: String,
    /// 本地路径
    pub local_path: String,
    /// 远端路径
    pub remote_path: String,
    /// 打包下载附加信息
    archive: Option<ArchiveRuntime>,
    /// 暂停、继续、取消及执行代际控制
    control: TaskControl,
    /// 已传输字节数
    pub transferred: AtomicU64,
    /// 总字节数
    pub total: AtomicU64,
    /// 当前速度（字节/秒，由进度循环计算）
    pub speed: AtomicU64,
    /// 预计剩余秒数（-1 表示未知）
    pub eta_secs: AtomicI64,
    /// 累计传输耗时（毫秒，暂停期间不累计）
    pub elapsed_ms: AtomicU64,
    /// 失败原因
    pub error: Mutex<String>,
    /// 是否已运行过（决定续传还是覆盖）
    pub started_once: AtomicBool,
    /// 同一任务执行体互斥，避免旧连接尚未退出时重试并发写半成品
    runner_lock: AsyncMutex<()>,
}

impl TaskState {
    /// 读取当前状态
    fn status(&self) -> u8 {
        self.control.status()
    }

    /// 状态转前端字符串
    fn status_str(&self) -> &'static str {
        self.control.status_str()
    }
}

/// 传输任务完整信息（transfer://changed 与 transfer_list 载荷）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferTaskDto {
    pub id: String,
    pub parent_id: Option<String>,
    pub session_id: String,
    pub kind: String,
    pub is_dir: bool,
    pub name: String,
    pub local_path: String,
    pub remote_path: String,
    pub status: String,
    pub transferred: u64,
    pub total: u64,
    pub speed: u64,
    pub eta_secs: i64,
    pub elapsed_ms: u64,
    pub error: String,
}

/// 传输任务动态字段（transfer://progress 载荷）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgressDto {
    pub id: String,
    pub status: String,
    pub transferred: u64,
    pub total: u64,
    pub speed: u64,
    pub eta_secs: i64,
    pub elapsed_ms: u64,
    pub error: String,
}

/// 创建任务的返回结果：超过阈值且未强制时不建任务，仅返回统计供前端确认
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferCreateResult {
    pub need_confirm: bool,
    /// 本次待传文件数
    pub file_count: u64,
    /// 会话内已存在的未完成任务数
    pub active_count: u64,
    /// 目标位置已存在的同名条目，非空时未建任务需前端确认覆盖
    pub exist_names: Vec<String>,
}

impl TransferCreateResult {
    /// 任务已创建的正常返回
    fn created(file_count: u64) -> Self {
        Self {
            need_confirm: false,
            file_count,
            active_count: 0,
            exist_names: Vec::new(),
        }
    }
}

/// 下载入参中的远端条目
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteItemArg {
    pub path: String,
    pub is_dir: bool,
}

/// 枚举得到的相对条目（相对路径统一正斜杠，父目录先于子内容）
struct ScanEntry {
    rel: String,
    is_dir: bool,
    size: u64,
}

/// 全局传输任务管理器，作为 Tauri 托管状态
pub struct TransferManager {
    /// 全部任务
    tasks: DashMap<String, Arc<TaskState>>,
    /// 目录任务的子任务标识列表
    children: DashMap<String, Vec<String>>,
    /// 任务展示顺序（父先于子）
    order: Mutex<Vec<String>>,
    /// 并发传输信号量
    semaphore: Arc<Semaphore>,
    /// 已确保存在的远端目录缓存（键为 sessionId + \n + 路径）
    dir_cache: DashSet<String>,
    /// 进度循环的速度跟踪（上次采样的字节数）
    speed_track: Mutex<HashMap<String, u64>>,
    /// 进度循环的上次推送快照，用于增量推送
    snapshot: Mutex<HashMap<String, (u8, u64, u64, u64)>>,
}

impl Default for TransferManager {
    fn default() -> Self {
        Self {
            tasks: DashMap::new(),
            children: DashMap::new(),
            order: Mutex::new(Vec::new()),
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT)),
            dir_cache: DashSet::new(),
            speed_track: Mutex::new(HashMap::new()),
            snapshot: Mutex::new(HashMap::new()),
        }
    }
}

/// 拼接远端路径（统一正斜杠）
fn join_remote(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), name)
    }
}

/// 取远端路径的父目录
fn remote_parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => trimmed[..idx].to_string(),
        None => "/".to_string(),
    }
}

/// 校验打包下载的本地目标，仅普通文件允许在用户确认后覆盖。
async fn pack_download_needs_overwrite(local_path: &Path, overwrite: bool) -> Result<bool> {
    let parent = local_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("下载目标缺少有效的父目录"))?;
    let parent_metadata = fs::metadata(parent)
        .await
        .map_err(|error| anyhow!("下载目录不可用：{}，{}", parent.display(), error))?;
    if !parent_metadata.is_dir() {
        return Err(anyhow!("下载路径不是文件夹：{}", parent.display()));
    }
    let metadata = match fs::symlink_metadata(local_path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(anyhow!("检查下载目标失败：{}", error)),
    };
    if metadata.is_dir() {
        return Err(anyhow!(
            "下载目标已存在同名文件夹，无法保存压缩包：{}。请更改下载路径或移走该文件夹后重试。",
            local_path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(anyhow!(
            "下载目标不是普通文件，无法覆盖：{}",
            local_path.display()
        ));
    }
    Ok(!overwrite)
}

impl TransferManager {
    /// 注册一个任务并纳入展示顺序
    fn register(&self, task: Arc<TaskState>) {
        if let Some(parent) = task.parent_id.clone() {
            self.children
                .entry(parent)
                .or_default()
                .push(task.id.clone());
        }
        self.order.lock().unwrap().push(task.id.clone());
        self.tasks.insert(task.id.clone(), task);
    }

    /// 构造任务对象
    #[allow(clippy::too_many_arguments)]
    fn new_task(
        &self,
        session_id: &str,
        parent_id: Option<String>,
        kind: TransferKind,
        is_dir: bool,
        name: String,
        local_path: String,
        remote_path: String,
        total: u64,
        archive: Option<ArchiveRuntime>,
    ) -> Arc<TaskState> {
        Arc::new(TaskState {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id,
            session_id: session_id.to_string(),
            kind,
            is_dir,
            name,
            local_path,
            remote_path,
            archive,
            control: TaskControl::new(ST_PENDING),
            transferred: AtomicU64::new(0),
            total: AtomicU64::new(total),
            speed: AtomicU64::new(0),
            eta_secs: AtomicI64::new(-1),
            elapsed_ms: AtomicU64::new(0),
            error: Mutex::new(String::new()),
            started_once: AtomicBool::new(false),
            runner_lock: AsyncMutex::new(()),
        })
    }

    /// 沿父链为祖先目录节点累加总字节数（用于创建时即显示目录总大小）
    fn add_total_to_ancestors(&self, mut parent_id: Option<String>, size: u64) {
        while let Some(pid) = parent_id {
            match self.tasks.get(&pid) {
                Some(parent) => {
                    parent.total.fetch_add(size, Ordering::SeqCst);
                    parent_id = parent.parent_id.clone();
                }
                None => break,
            }
        }
    }

    /// 清理指定会话的远端目录存在缓存
    ///
    /// 缓存仅用于同一次上传内多文件共享祖先目录、避免重复请求；跨次上传之间远端目录
    /// 可能已被删除，若沿用旧缓存会跳过重建导致写入子文件时报 No such file，
    /// 故每次上传开始前先清空该会话的缓存。
    fn invalidate_dir_cache(&self, session_id: &str) {
        let prefix = format!("{}\n", session_id);
        self.dir_cache.retain(|key| !key.starts_with(&prefix));
    }

    /// 统计指定会话内未完成的文件任务数（不含聚合目录节点，不分上传下载）
    fn active_file_count(&self, session_id: &str) -> u64 {
        self.tasks
            .iter()
            .filter(|t| {
                let status = t.status();
                t.session_id == session_id
                    && !t.is_dir
                    && status != ST_COMPLETED
                    && status != ST_CANCELLED
            })
            .count() as u64
    }

    /// 校验本次文件数量（与会话内未完成任务合计）：超过上限时拒绝，超过确认阈值且未强制时要求确认
    fn check_file_count(
        &self,
        session_id: &str,
        file_count: u64,
        force: bool,
    ) -> Result<Option<TransferCreateResult>> {
        let active = self.active_file_count(session_id);
        let total = file_count + active;
        if total > MAX_TOTAL_FILES {
            if active > 0 {
                return Err(anyhow!(
                    "本次共 {} 个文件，加上传输中的 {} 个任务已超过最大 {} 个文件限制，推荐打包压缩后传输",
                    file_count, active, MAX_TOTAL_FILES
                ));
            }
            return Err(anyhow!(
                "本次共 {} 个文件，超过最大 {} 个文件限制，推荐打包压缩后传输",
                file_count,
                MAX_TOTAL_FILES
            ));
        }
        if !force && total > CONFIRM_THRESHOLD {
            return Ok(Some(TransferCreateResult {
                need_confirm: true,
                file_count,
                active_count: active,
                exist_names: Vec::new(),
            }));
        }
        Ok(None)
    }

    /// 创建上传任务：枚举本地路径，超过阈值且未强制时仅返回统计
    pub async fn create_upload(
        &self,
        app: &AppHandle,
        session_id: &str,
        local_paths: Vec<String>,
        remote_dir: String,
        force: bool,
        overwrite: bool,
    ) -> Result<TransferCreateResult> {
        // 本地枚举放入阻塞线程，避免大目录卡住异步运行时
        let paths = local_paths.clone();
        let scans = tokio::task::spawn_blocking(move || scan_local_roots(&paths))
            .await
            .map_err(|e| anyhow!("枚举本地文件失败：{}", e))??;

        let file_count: u64 = scans
            .iter()
            .map(|(_, _, _, entries)| match entries {
                Some(list) => list.iter().filter(|e| !e.is_dir).count() as u64,
                None => 1,
            })
            .sum();
        if let Some(result) = self.check_file_count(session_id, file_count, force)? {
            return Ok(result);
        }

        // 清空本会话残留的远端目录缓存，避免复用已被删除目录的旧记录
        self.invalidate_dir_cache(session_id);

        // 目标位置同名检测：未确认覆盖时收集已存在的顶层条目名返回前端确认
        if !overwrite {
            let manager = app.state::<SessionManager>();
            let sftp = manager.sftp(session_id).await?;
            let mut exist_names = Vec::new();
            for (_, root_name, _, _) in &scans {
                let remote_root = join_remote(&remote_dir, root_name);
                if sftp.metadata(&remote_root).await.is_ok() {
                    exist_names.push(root_name.clone());
                }
            }
            if !exist_names.is_empty() {
                return Ok(TransferCreateResult {
                    need_confirm: false,
                    file_count,
                    active_count: 0,
                    exist_names,
                });
            }
        }

        for (root_path, root_name, root_is_dir, entries) in scans {
            let remote_root = join_remote(&remote_dir, &root_name);
            if !root_is_dir {
                // 顶层文件：单个任务
                let size = std::fs::metadata(&root_path).map(|m| m.len()).unwrap_or(0);
                let task = self.new_task(
                    session_id,
                    None,
                    TransferKind::Upload,
                    false,
                    root_name,
                    root_path.to_string_lossy().to_string(),
                    remote_root,
                    size,
                    None,
                );
                self.register(task.clone());
                spawn_file_runner(app.clone(), task);
                continue;
            }
            // 顶层目录：根节点 + 子树
            let root_task = self.new_task(
                session_id,
                None,
                TransferKind::Upload,
                true,
                root_name,
                root_path.to_string_lossy().to_string(),
                remote_root.clone(),
                0,
                None,
            );
            let root_id = root_task.id.clone();
            self.register(root_task);
            // 相对目录路径 -> 任务标识，用于挂接子项
            let mut dir_ids: HashMap<String, String> = HashMap::new();
            dir_ids.insert(String::new(), root_id.clone());
            for entry in entries.unwrap_or_default() {
                let parent_rel = match entry.rel.rfind('/') {
                    Some(idx) => entry.rel[..idx].to_string(),
                    None => String::new(),
                };
                let parent_id = dir_ids
                    .get(&parent_rel)
                    .cloned()
                    .unwrap_or_else(|| root_id.clone());
                let name = entry
                    .rel
                    .rsplit('/')
                    .next()
                    .unwrap_or(&entry.rel)
                    .to_string();
                let local = root_path.join(entry.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                let remote = format!("{}/{}", remote_root.trim_end_matches('/'), entry.rel);
                let task = self.new_task(
                    session_id,
                    Some(parent_id.clone()),
                    TransferKind::Upload,
                    entry.is_dir,
                    name,
                    local.to_string_lossy().to_string(),
                    remote,
                    if entry.is_dir { 0 } else { entry.size },
                    None,
                );
                if entry.is_dir {
                    dir_ids.insert(entry.rel.clone(), task.id.clone());
                }
                self.register(task.clone());
                if !entry.is_dir {
                    self.add_total_to_ancestors(task.parent_id.clone(), entry.size);
                    spawn_file_runner(app.clone(), task);
                }
            }
            // 空目录没有子任务，需要单独创建远端目录
            for id in dir_ids.values() {
                if self
                    .children
                    .get(id)
                    .map(|c| !c.is_empty())
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(task) = self.tasks.get(id).map(|t| t.clone()) {
                    spawn_dir_creator(app.clone(), task);
                }
            }
        }
        self.emit_changed(app);
        Ok(TransferCreateResult::created(file_count))
    }

    /// 创建下载任务：枚举远端路径，超过阈值且未强制时仅返回统计
    pub async fn create_download(
        &self,
        app: &AppHandle,
        session_id: &str,
        items: Vec<RemoteItemArg>,
        local_dir: String,
        force: bool,
        overwrite: bool,
    ) -> Result<TransferCreateResult> {
        let manager = app.state::<SessionManager>();
        let sftp = manager.sftp(session_id).await?;

        // 逐项枚举远端条目
        let mut scans: Vec<(RemoteItemArg, Option<Vec<ScanEntry>>, u64)> = Vec::new();
        let mut file_count: u64 = 0;
        for item in items {
            if item.is_dir {
                let entries = scan_remote_dir(&sftp, &item.path).await?;
                file_count += entries.iter().filter(|e| !e.is_dir).count() as u64;
                scans.push((item, Some(entries), 0));
            } else {
                let size = sftp
                    .metadata(&item.path)
                    .await
                    .ok()
                    .and_then(|m| m.size)
                    .unwrap_or(0);
                file_count += 1;
                scans.push((item, None, size));
            }
        }
        if let Some(result) = self.check_file_count(session_id, file_count, force)? {
            return Ok(result);
        }

        let local_root_dir = PathBuf::from(&local_dir);

        // 本地同名检测：未确认覆盖时收集已存在的顶层条目名返回前端确认
        if !overwrite {
            let mut exist_names = Vec::new();
            for (item, _, _) in &scans {
                let name = item
                    .path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&item.path)
                    .to_string();
                if local_root_dir.join(&name).exists() {
                    exist_names.push(name);
                }
            }
            if !exist_names.is_empty() {
                return Ok(TransferCreateResult {
                    need_confirm: false,
                    file_count,
                    active_count: 0,
                    exist_names,
                });
            }
        }

        for (item, entries, size) in scans {
            let name = item
                .path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&item.path)
                .to_string();
            let local_root = local_root_dir.join(&name);
            if !item.is_dir {
                let task = self.new_task(
                    session_id,
                    None,
                    TransferKind::Download,
                    false,
                    name,
                    local_root.to_string_lossy().to_string(),
                    item.path.clone(),
                    size,
                    None,
                );
                self.register(task.clone());
                spawn_file_runner(app.clone(), task);
                continue;
            }
            let root_task = self.new_task(
                session_id,
                None,
                TransferKind::Download,
                true,
                name,
                local_root.to_string_lossy().to_string(),
                item.path.clone(),
                0,
                None,
            );
            let root_id = root_task.id.clone();
            self.register(root_task);
            let mut dir_ids: HashMap<String, String> = HashMap::new();
            dir_ids.insert(String::new(), root_id.clone());
            for entry in entries.unwrap_or_default() {
                let parent_rel = match entry.rel.rfind('/') {
                    Some(idx) => entry.rel[..idx].to_string(),
                    None => String::new(),
                };
                let parent_id = dir_ids
                    .get(&parent_rel)
                    .cloned()
                    .unwrap_or_else(|| root_id.clone());
                let entry_name = entry
                    .rel
                    .rsplit('/')
                    .next()
                    .unwrap_or(&entry.rel)
                    .to_string();
                let local = local_root.join(entry.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                let remote = format!("{}/{}", item.path.trim_end_matches('/'), entry.rel);
                let task = self.new_task(
                    session_id,
                    Some(parent_id.clone()),
                    TransferKind::Download,
                    entry.is_dir,
                    entry_name,
                    local.to_string_lossy().to_string(),
                    remote,
                    if entry.is_dir { 0 } else { entry.size },
                    None,
                );
                if entry.is_dir {
                    dir_ids.insert(entry.rel.clone(), task.id.clone());
                }
                self.register(task.clone());
                if !entry.is_dir {
                    self.add_total_to_ancestors(task.parent_id.clone(), entry.size);
                    spawn_file_runner(app.clone(), task);
                }
            }
            // 空目录在本地直接创建
            for id in dir_ids.values() {
                if self
                    .children
                    .get(id)
                    .map(|c| !c.is_empty())
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(task) = self.tasks.get(id).map(|t| t.clone()) {
                    spawn_dir_creator(app.clone(), task);
                }
            }
        }
        self.emit_changed(app);
        Ok(TransferCreateResult::created(file_count))
    }

    /// 创建打包下载任务：远端 tar 打包后作为单文件下载，完成后清理远端临时包
    pub async fn create_pack_download(
        &self,
        app: &AppHandle,
        session_id: &str,
        remote_dir: String,
        names: Vec<String>,
        local_path: String,
        overwrite: bool,
    ) -> Result<TransferCreateResult> {
        if names.is_empty() {
            return Err(anyhow!("未选择需要打包的文件"));
        }
        let file_name = Path::new(&local_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "archive.tar.gz".to_string());
        // 未确认覆盖时只返回冲突，不执行远端命令或登记传输任务。
        if pack_download_needs_overwrite(Path::new(&local_path), overwrite).await? {
            return Ok(TransferCreateResult {
                need_confirm: false,
                file_count: 1,
                active_count: 0,
                exist_names: vec![file_name],
            });
        }
        let manager = app.state::<SessionManager>();
        // 先探测远端 tar 命令是否可用，失败时立刻反馈
        let probe = manager
            .exec(
                session_id,
                "command -v tar >/dev/null 2>&1 && printf __ZTOK__ || printf __ZTNO__",
            )
            .await?;
        if !probe.contains("__ZTOK__") {
            return Err(anyhow!("远端未找到 tar 命令，无法打包下载"));
        }
        let task = self.new_task(
            session_id,
            None,
            TransferKind::Download,
            false,
            file_name,
            local_path,
            remote_dir.clone(),
            0,
            Some(ArchiveRuntime::new(remote_dir, names)),
        );
        self.register(task.clone());
        spawn_file_runner(app.clone(), task);
        self.emit_changed(app);
        Ok(TransferCreateResult::created(1))
    }

    /// 列出全部任务（按创建顺序）
    pub fn list(&self) -> Vec<TransferTaskDto> {
        let order = self.order.lock().unwrap().clone();
        order
            .iter()
            .filter_map(|id| self.tasks.get(id).map(|t| self.to_dto(&t)))
            .collect()
    }

    /// 任务转完整 DTO
    fn to_dto(&self, task: &TaskState) -> TransferTaskDto {
        TransferTaskDto {
            id: task.id.clone(),
            parent_id: task.parent_id.clone(),
            session_id: task.session_id.clone(),
            kind: task.kind.as_str().to_string(),
            is_dir: task.is_dir,
            name: task.name.clone(),
            local_path: task.local_path.clone(),
            remote_path: task.remote_path.clone(),
            status: task.status_str().to_string(),
            transferred: task.transferred.load(Ordering::SeqCst),
            total: task.total.load(Ordering::SeqCst),
            speed: task.speed.load(Ordering::SeqCst),
            eta_secs: task.eta_secs.load(Ordering::SeqCst),
            elapsed_ms: task.elapsed_ms.load(Ordering::SeqCst),
            error: task.error.lock().unwrap().clone(),
        }
    }

    /// 推送任务结构变化事件（全量列表）
    pub fn emit_changed(&self, app: &AppHandle) {
        let _ = app.emit("transfer://changed", self.list());
    }

    /// 收集指定任务（含目录时展开子树）；ids 为空时收集全部
    ///
    /// include_dirs 为 false 时仅返回执行单元（文件任务与空目录任务），用于暂停/继续；
    /// 为 true 时包含聚合目录节点，用于删除
    fn collect_targets(&self, ids: Option<Vec<String>>, include_dirs: bool) -> Vec<Arc<TaskState>> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        let roots = match ids {
            Some(list) => list,
            None => self.order.lock().unwrap().clone(),
        };
        for id in roots {
            self.collect_subtree(&id, include_dirs, &mut visited, &mut out);
        }
        out
    }

    /// 递归收集子树任务
    fn collect_subtree(
        &self,
        id: &str,
        include_dirs: bool,
        visited: &mut HashSet<String>,
        out: &mut Vec<Arc<TaskState>>,
    ) {
        if !visited.insert(id.to_string()) {
            return;
        }
        let Some(task) = self.tasks.get(id).map(|t| t.clone()) else {
            return;
        };
        let kids = self.children.get(id).map(|c| c.clone()).unwrap_or_default();
        let is_aggregate = task.is_dir && !kids.is_empty();
        if include_dirs || !is_aggregate {
            out.push(task);
        }
        for kid in kids {
            self.collect_subtree(&kid, include_dirs, visited, out);
        }
    }

    /// 暂停任务：排队中的直接置为暂停，运行中的下发暂停指令由执行体落地
    pub fn pause(&self, app: &AppHandle, ids: Option<Vec<String>>) {
        for task in self.collect_targets(ids, false) {
            request_pause(&task);
        }
        self.emit_changed(app);
    }

    /// 继续任务：已暂停的重新入队；暂停请求尚未落地时由当前执行体继续
    pub fn resume(&self, app: &AppHandle, ids: Option<Vec<String>>) {
        for task in self.collect_targets(ids, false) {
            if !matches!(task.control.resume(), ResumeAction::Requeue) {
                continue;
            }
            task.error.lock().unwrap().clear();
            if task.is_dir {
                spawn_dir_creator(app.clone(), task.clone());
            } else {
                spawn_file_runner(app.clone(), task.clone());
            }
        }
        self.emit_changed(app);
    }

    /// 删除任务（级联子树）：取消执行并从列表移除，同时清理已登记的远端临时包
    pub fn remove(&self, app: &AppHandle, ids: Option<Vec<String>>) {
        let targets = self.collect_targets(ids, true);
        let removed: HashSet<String> = targets.iter().map(|t| t.id.clone()).collect();
        for task in &targets {
            task.control.cancel();
            if task.archive.is_some() {
                let manager = app.state::<SessionManager>();
                let operation_id = format!("transfer-pack:{}", task.id);
                let _ = manager.cancel_operation(&task.session_id, &operation_id);
            }
            self.tasks.remove(&task.id);
            self.children.remove(&task.id);
            // 已发布的临时包路径不会再被旧打包尝试覆盖，可以立即清理。
            let ready_archive = task.archive.as_ref().and_then(ArchiveRuntime::take_ready);
            if let Some(ready_archive) = ready_archive {
                let app = app.clone();
                let session_id = task.session_id.clone();
                tauri::async_runtime::spawn(async move {
                    cleanup_remote_tmp(&app, &session_id, &ready_archive.path).await;
                });
            }
        }
        // 维护展示顺序与父子索引
        self.order
            .lock()
            .unwrap()
            .retain(|id| !removed.contains(id));
        for mut entry in self.children.iter_mut() {
            entry.value_mut().retain(|id| !removed.contains(id));
        }
        {
            let mut track = self.speed_track.lock().unwrap();
            let mut snap = self.snapshot.lock().unwrap();
            for id in &removed {
                track.remove(id);
                snap.remove(id);
            }
        }
        self.emit_changed(app);
    }

    /// 重试失败的任务（断点续传接续已传部分），可按会话及任务标识限制范围
    pub fn retry_failed(
        &self,
        app: &AppHandle,
        session_id: Option<&str>,
        ids: Option<Vec<String>>,
    ) {
        for task in self.collect_targets(ids, false) {
            if let Some(sid) = session_id {
                if task.session_id != sid {
                    continue;
                }
            }
            let mut error = task.error.lock().unwrap();
            if !task.control.retry_failed() {
                continue;
            }
            error.clear();
            drop(error);
            if task.is_dir {
                spawn_dir_creator(app.clone(), task.clone());
            } else {
                spawn_file_runner(app.clone(), task.clone());
            }
        }
        self.emit_changed(app);
    }

    /// 标记指定会话的任务已中断并保留列表，供会话重连后按断点重试
    pub fn interrupt_session(&self, app: &AppHandle, session_id: &str) {
        self.invalidate_dir_cache(session_id);
        let mut changed = false;
        for task in self.tasks.iter() {
            if task.session_id != session_id {
                continue;
            }
            changed |= mark_task_interrupted(&task);
        }
        if changed {
            self.emit_changed(app);
        }
    }

    /// 移除指定会话的全部传输任务（用户关闭选项卡时调用）
    pub fn remove_session(&self, app: &AppHandle, session_id: &str) {
        self.invalidate_dir_cache(session_id);
        let ids: Vec<String> = self
            .order
            .lock()
            .unwrap()
            .iter()
            .filter(|id| {
                self.tasks
                    .get(*id)
                    .map(|t| t.session_id == session_id)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if !ids.is_empty() {
            self.remove(app, Some(ids));
        }
    }

    /// 进度循环单次采样：计算速度、聚合目录、累计耗时，返回相对上次的增量
    fn collect_progress(&self, dt_ms: u64) -> Vec<TransferProgressDto> {
        let order = self.order.lock().unwrap().clone();
        if order.is_empty() {
            self.speed_track.lock().unwrap().clear();
            self.snapshot.lock().unwrap().clear();
            return Vec::new();
        }
        let dt_secs = (dt_ms as f64 / 1000.0).max(0.001);

        // 第一步：计算执行单元（非聚合节点）在本周期内的传输速度
        {
            let mut track = self.speed_track.lock().unwrap();
            for id in &order {
                let Some(task) = self.tasks.get(id) else {
                    continue;
                };
                let has_children = self
                    .children
                    .get(id)
                    .map(|c| !c.is_empty())
                    .unwrap_or(false);
                if task.is_dir && has_children {
                    continue;
                }
                let cur = task.transferred.load(Ordering::SeqCst);
                let previous = track.insert(id.clone(), cur).unwrap_or(0);
                let speed = if task.status() == ST_RUNNING {
                    ((cur.saturating_sub(previous)) as f64 / dt_secs) as u64
                } else {
                    0
                };
                task.speed.store(speed, Ordering::SeqCst);
            }
        }

        // 第二步：自底向上聚合目录节点（order 保证父先于子，反向遍历即自底向上）
        for id in order.iter().rev() {
            let Some(task) = self.tasks.get(id).map(|t| t.clone()) else {
                continue;
            };
            let kids = self.children.get(id).map(|c| c.clone()).unwrap_or_default();
            if !task.is_dir || kids.is_empty() {
                continue;
            }
            let mut transferred = 0u64;
            let mut total = 0u64;
            let mut speed = 0u64;
            let mut has = [false; 7];
            for kid in &kids {
                let Some(child) = self.tasks.get(kid) else {
                    continue;
                };
                transferred += child.transferred.load(Ordering::SeqCst);
                total += child.total.load(Ordering::SeqCst);
                speed += child.speed.load(Ordering::SeqCst);
                has[child.status() as usize] = true;
            }
            // 聚合状态优先级：传输中 > 打包中 > 等待中 > 已暂停 > 失败 > 已完成
            let status = if has[ST_RUNNING as usize] {
                ST_RUNNING
            } else if has[ST_PACKING as usize] {
                ST_PACKING
            } else if has[ST_PENDING as usize] {
                ST_PENDING
            } else if has[ST_PAUSED as usize] {
                ST_PAUSED
            } else if has[ST_FAILED as usize] {
                ST_FAILED
            } else {
                ST_COMPLETED
            };
            task.transferred.store(transferred, Ordering::SeqCst);
            task.total.store(total, Ordering::SeqCst);
            task.speed.store(speed, Ordering::SeqCst);
            task.control.set_aggregate_status(status);
        }

        // 第三步：累计耗时、计算预计剩余，并与上次快照比对生成增量
        let mut updates = Vec::new();
        let mut snapshot = self.snapshot.lock().unwrap();
        for id in &order {
            let Some(task) = self.tasks.get(id) else {
                continue;
            };
            let status = task.status();
            if status == ST_RUNNING || status == ST_PACKING {
                task.elapsed_ms.fetch_add(dt_ms, Ordering::SeqCst);
            }
            let transferred = task.transferred.load(Ordering::SeqCst);
            let total = task.total.load(Ordering::SeqCst);
            let speed = task.speed.load(Ordering::SeqCst);
            let eta = if status == ST_RUNNING && speed > 0 && total > transferred {
                ((total - transferred) / speed) as i64
            } else {
                -1
            };
            task.eta_secs.store(eta, Ordering::SeqCst);
            let elapsed = task.elapsed_ms.load(Ordering::SeqCst);
            let snap_val = (status, transferred, speed, elapsed);
            if snapshot.get(id) != Some(&snap_val) {
                snapshot.insert(id.clone(), snap_val);
                updates.push(TransferProgressDto {
                    id: id.clone(),
                    status: status_str(status).to_string(),
                    transferred,
                    total,
                    speed,
                    eta_secs: eta,
                    elapsed_ms: elapsed,
                    error: task.error.lock().unwrap().clone(),
                });
            }
        }
        updates
    }
}

/// 将一个仍在排队或执行中的任务原子地转为可重试的失败状态。
///
/// 先递增执行代际再 CAS 状态，避免旧执行体在断线竞态中把任务错误写回已完成。
fn mark_task_interrupted(task: &TaskState) -> bool {
    let mut error = task.error.lock().unwrap();
    if !task.control.interrupt() {
        return false;
    }
    *error = SESSION_INTERRUPTED_MESSAGE.to_string();
    task.speed.store(0, Ordering::SeqCst);
    task.eta_secs.store(-1, Ordering::SeqCst);
    true
}

/// 请求暂停任务；状态从等待中切换到执行中的瞬间会重新检查，避免丢失暂停请求。
fn request_pause(task: &TaskState) {
    task.control.request_pause();
}

/// 将当前执行体的活动状态转为失败并记录错误，兼容普通传输和打包阶段。
fn mark_runner_failed(task: &TaskState, generation: u64, message: &str) -> bool {
    let mut error = task.error.lock().unwrap();
    if !task.control.fail(generation) {
        return false;
    }
    *error = message.to_string();
    true
}

/// 启动进度推送循环（应用启动时调用一次）
pub fn start_progress_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
            let now = Instant::now();
            let dt_ms = now.duration_since(last).as_millis() as u64;
            last = now;
            let tm = app.state::<TransferManager>();
            let updates = tm.collect_progress(dt_ms);
            if !updates.is_empty() {
                let _ = app.emit("transfer://progress", &updates);
            }
        }
    });
}

/// 枚举本地顶层路径，目录时递归收集相对条目（父目录先于子内容）
#[allow(clippy::type_complexity)]
fn scan_local_roots(
    paths: &[String],
) -> Result<Vec<(PathBuf, String, bool, Option<Vec<ScanEntry>>)>> {
    let mut out = Vec::new();
    for raw in paths {
        let path = PathBuf::from(raw);
        let meta =
            std::fs::metadata(&path).map_err(|e| anyhow!("读取本地路径失败（{}）：{}", raw, e))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "未命名".to_string());
        if meta.is_dir() {
            let entries = scan_local_dir(&path)?;
            out.push((path, name, true, Some(entries)));
        } else {
            out.push((path, name, false, None));
        }
    }
    Ok(out)
}

/// 递归枚举本地目录，返回相对条目列表
fn scan_local_dir(root: &Path) -> Result<Vec<ScanEntry>> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let abs = root.join(&rel);
        let read = std::fs::read_dir(&abs)
            .map_err(|e| anyhow!("读取本地目录失败（{}）：{}", abs.display(), e))?;
        for entry in read {
            let entry = entry.map_err(|e| anyhow!("读取本地目录项失败：{}", e))?;
            let child_rel = rel.join(entry.file_name());
            let rel_str = child_rel.to_string_lossy().replace('\\', "/");
            let file_type = entry
                .file_type()
                .map_err(|e| anyhow!("读取文件类型失败：{}", e))?;
            if file_type.is_dir() {
                out.push(ScanEntry {
                    rel: rel_str,
                    is_dir: true,
                    size: 0,
                });
                stack.push(child_rel);
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(ScanEntry {
                    rel: rel_str,
                    is_dir: false,
                    size,
                });
            }
        }
    }
    Ok(out)
}

/// 递归枚举远端目录，返回相对条目列表（符号链接按文件处理，不深入避免循环）
async fn scan_remote_dir(sftp: &SftpSession, root: &str) -> Result<Vec<ScanEntry>> {
    let mut out = Vec::new();
    let mut stack: Vec<String> = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let abs = if rel.is_empty() {
            root.to_string()
        } else {
            format!("{}/{}", root.trim_end_matches('/'), rel)
        };
        let read = sftp
            .read_dir(&abs)
            .await
            .map_err(|e| anyhow!("读取远端目录失败（{}）：{}", abs, format_sftp_error(&e)))?;
        for item in read {
            let name = item.file_name();
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", rel, name)
            };
            let meta = item.metadata();
            if matches!(meta.file_type(), russh_sftp::protocol::FileType::Dir) {
                out.push(ScanEntry {
                    rel: child_rel.clone(),
                    is_dir: true,
                    size: 0,
                });
                stack.push(child_rel);
            } else {
                out.push(ScanEntry {
                    rel: child_rel,
                    is_dir: false,
                    size: meta.size.unwrap_or(0),
                });
            }
        }
    }
    Ok(out)
}

/// 确保远端目录逐级存在（带缓存避免重复请求）
async fn ensure_remote_dir(
    tm: &TransferManager,
    sftp: &SftpSession,
    session_id: &str,
    dir: &str,
) -> Result<()> {
    if dir.is_empty() || dir == "/" {
        return Ok(());
    }
    // 从浅到深逐级检查
    let mut prefix = String::new();
    for part in dir.split('/').filter(|p| !p.is_empty()) {
        prefix = format!("{}/{}", prefix, part);
        let key = format!("{}\n{}", session_id, prefix);
        if tm.dir_cache.contains(&key) {
            continue;
        }
        if sftp.metadata(&prefix).await.is_ok() {
            tm.dir_cache.insert(key);
            continue;
        }
        if let Err(e) = sftp.create_dir(&prefix).await {
            // 并发场景下可能已被其他任务创建，再确认一次
            if sftp.metadata(&prefix).await.is_err() {
                return Err(anyhow!(
                    "创建远端目录失败（{}）：{}",
                    prefix,
                    format_sftp_error(&e)
                ));
            }
        }
        tm.dir_cache.insert(key);
    }
    Ok(())
}

/// 检查控制指令：收到暂停请求时落地为暂停状态；返回 false 表示应停止传输
fn check_control(task: &TaskState, generation: u64) -> bool {
    task.control.checkpoint(generation)
}

/// 判断活动执行体是否收到暂停、取消或代际失效请求。
fn task_control_requested(task: &TaskState, generation: u64) -> bool {
    task.control.stop_requested(generation)
}

/// 等待并原子落地任务控制请求；暂停已被继续撤销时保持当前 I/O 等待。
async fn wait_for_task_control(task: &TaskState, generation: u64) {
    let mut changes = task.control.subscribe();
    loop {
        if task_control_requested(task, generation) && !check_control(task, generation) {
            return;
        }
        if changes.changed().await.is_err() {
            return;
        }
    }
}

/// 在任务控制通知与有界异步操作之间竞争，None 表示当前执行体应停止。
async fn await_with_control<F>(
    task: &TaskState,
    generation: u64,
    max_wait: Duration,
    operation: F,
) -> Option<Result<F::Output, Elapsed>>
where
    F: Future,
{
    tokio::select! {
        biased;
        _ = wait_for_task_control(task, generation) => None,
        result = timeout(max_wait, operation) => Some(result),
    }
}

/// 将带业务错误的有界操作统一转换为可中断结果。
async fn await_result_with_control<F, T, E, M>(
    task: &TaskState,
    generation: u64,
    max_wait: Duration,
    operation: F,
    timeout_message: &'static str,
    map_error: M,
) -> Result<Option<T>>
where
    F: Future<Output = std::result::Result<T, E>>,
    M: FnOnce(E) -> Error,
{
    match await_with_control(task, generation, max_wait, operation).await {
        None => Ok(None),
        Some(Err(_)) => Err(anyhow!(timeout_message)),
        Some(Ok(Err(error))) => Err(map_error(error)),
        Some(Ok(Ok(value))) => Ok(Some(value)),
    }
}

/// 判断当前执行体是否已被取消或因会话变化失效，但忽略等待打包完成的暂停请求。
fn task_cancelled(task: &TaskState, generation: u64) -> bool {
    task.control.cancelled(generation)
}

/// 等待删除、断线或重试令当前执行体失效。
async fn wait_for_task_cancellation(task: &TaskState, generation: u64) {
    let mut changes = task.control.subscribe();
    while !task_cancelled(task, generation) {
        if changes.changed().await.is_err() {
            return;
        }
    }
}

/// 获取当前会话的 SFTP 客户端，允许暂停、取消或代际变化中断等待。
async fn sftp_with_control(
    manager: &SessionManager,
    task: &TaskState,
    generation: u64,
) -> Result<Option<Arc<SftpSession>>> {
    await_result_with_control(
        task,
        generation,
        SFTP_REQUEST_TIMEOUT,
        manager.sftp(&task.session_id),
        "建立 SFTP 会话超时",
        |error| error,
    )
    .await
}

/// 可中断的重试等待：期间响应暂停/取消，返回 false 表示应终止
async fn sleep_with_control(task: &TaskState, generation: u64, ms: u64) -> bool {
    let steps = ms / 100;
    for _ in 0..steps {
        if !check_control(task, generation) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    check_control(task, generation)
}

/// 启动空目录任务：上传方向创建远端目录，下载方向创建本地目录
fn spawn_dir_creator(app: AppHandle, task: Arc<TaskState>) {
    tauri::async_runtime::spawn(async move {
        let _runner_guard = task.runner_lock.lock().await;
        let generation = task.control.generation();
        if !task.control.start(generation) {
            return;
        }
        let result: Result<()> = async {
            match task.kind {
                TransferKind::Upload => {
                    let manager = app.state::<SessionManager>();
                    let sftp = manager.sftp(&task.session_id).await?;
                    let tm = app.state::<TransferManager>();
                    ensure_remote_dir(&tm, &sftp, &task.session_id, &task.remote_path).await
                }
                TransferKind::Download => tokio::fs::create_dir_all(&task.local_path)
                    .await
                    .map_err(|error| anyhow!("创建本地目录失败：{}", error)),
            }
        }
        .await;
        if !task.control.is_generation(generation) {
            return;
        }
        match result {
            Ok(()) => {
                task.control.complete(generation);
            }
            Err(error) => {
                let mut message = task.error.lock().unwrap();
                if task.control.fail(generation) {
                    *message = error.to_string();
                }
            }
        }
    });
}

/// 启动文件传输任务执行体：受并发信号量约束，失败自动重试
fn spawn_file_runner(app: AppHandle, task: Arc<TaskState>) {
    tauri::async_runtime::spawn(async move {
        let generation = task.control.generation();
        let mut control_changes = task.control.subscribe();
        let _runner_guard = loop {
            if !task.control.is_pending_generation(generation) {
                return;
            }
            tokio::select! {
                biased;
                changed = control_changes.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                guard = task.runner_lock.lock() => break guard,
            }
        };
        let semaphore = app.state::<TransferManager>().semaphore.clone();
        let _permit = loop {
            if !task.control.is_pending_generation(generation) {
                return;
            }
            tokio::select! {
                biased;
                changed = control_changes.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                permit = semaphore.clone().acquire_owned() => {
                    let Ok(permit) = permit else {
                        return;
                    };
                    break permit;
                }
            }
        };
        if !task.control.start(generation) {
            return;
        }

        let mut attempt: u32 = 0;
        loop {
            let result = if task.archive.is_some() {
                run_archive_once(&app, &task, generation).await
            } else {
                match task.kind {
                    TransferKind::Upload => run_upload_once(&app, &task, generation).await,
                    TransferKind::Download => run_download_once(&app, &task, generation).await,
                }
            };
            if !task.control.is_generation(generation) {
                break;
            }
            match result {
                Ok(true) => {
                    task.control.complete(generation);
                    break;
                }
                // 被暂停或取消，状态已在检查点落地
                Ok(false) => break,
                Err(error) => {
                    // 暂停或取消可能在最后一次网络等待期间到达，优先落地控制请求。
                    if !check_control(&task, generation) {
                        break;
                    }
                    attempt += 1;
                    if attempt >= MAX_ATTEMPTS {
                        mark_runner_failed(&task, generation, &error.to_string());
                        break;
                    }
                    // 网络波动自动重试，重试后按已落盘字节续传
                    if !sleep_with_control(&task, generation, RETRY_DELAY_MS).await {
                        break;
                    }
                    let status = task.status();
                    if status == ST_PACKING {
                        if !task.control.finish_packing(generation) {
                            break;
                        }
                    } else if status != ST_RUNNING {
                        break;
                    }
                }
            }
        }
    });
}

/// 执行一次上传：返回 Ok(true) 完成、Ok(false) 被暂停/取消、Err 出错待重试
async fn run_upload_once(app: &AppHandle, task: &Arc<TaskState>, generation: u64) -> Result<bool> {
    let manager = app.state::<SessionManager>();
    let Some(sftp) = sftp_with_control(&manager, task, generation).await? else {
        return Ok(false);
    };
    let tm = app.state::<TransferManager>();
    ensure_remote_dir(
        &tm,
        &sftp,
        &task.session_id,
        &remote_parent(&task.remote_path),
    )
    .await?;

    let total = tokio::fs::metadata(&task.local_path)
        .await
        .map_err(|e| anyhow!("读取本地文件失败：{}", e))?
        .len();
    task.total.store(total, Ordering::SeqCst);

    // 断点定位：首次运行覆盖写，重试/继续时按远端已落盘字节续传
    let mut offset = 0u64;
    if task.started_once.load(Ordering::SeqCst) {
        if let Ok(meta) = sftp.metadata(&task.remote_path).await {
            let remote_len = meta.size.unwrap_or(0);
            if remote_len <= total {
                offset = remote_len;
            }
        }
    }
    task.started_once.store(true, Ordering::SeqCst);
    if total > 0 && offset >= total {
        task.transferred.store(total, Ordering::SeqCst);
        return Ok(true);
    }

    let mut local = tokio::fs::File::open(&task.local_path)
        .await
        .map_err(|e| anyhow!("打开本地文件失败：{}", e))?;
    let flags = if offset > 0 {
        OpenFlags::WRITE | OpenFlags::CREATE
    } else {
        OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE
    };
    let mut remote = sftp
        .open_with_flags(&task.remote_path, flags)
        .await
        .map_err(|e| anyhow!("打开远端文件失败：{}", format_sftp_error(&e)))?;
    if offset > 0 {
        local
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| anyhow!("定位本地文件失败：{}", e))?;
        remote
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| anyhow!("定位远端文件失败：{}", e))?;
    }
    task.transferred.store(offset, Ordering::SeqCst);

    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        if !check_control(task, generation) {
            let _ = remote.shutdown().await;
            return Ok(false);
        }
        let n = local
            .read(&mut buf)
            .await
            .map_err(|e| anyhow!("读取本地文件失败：{}", e))?;
        if n == 0 {
            break;
        }
        remote
            .write_all(&buf[..n])
            .await
            .map_err(|e| anyhow!("写入远端文件失败：{}", e))?;
        task.transferred.fetch_add(n as u64, Ordering::SeqCst);
    }
    remote
        .flush()
        .await
        .map_err(|e| anyhow!("刷新远端文件失败：{}", e))?;
    let _ = remote.shutdown().await;
    Ok(true)
}

/// 执行一次下载：返回含义同上传
async fn run_download_once(
    app: &AppHandle,
    task: &Arc<TaskState>,
    generation: u64,
) -> Result<bool> {
    let manager = app.state::<SessionManager>();
    let Some(sftp) = sftp_with_control(&manager, task, generation).await? else {
        return Ok(false);
    };
    stream_download(&sftp, task, &task.remote_path.clone(), generation).await
}

/// 下载核心：远端文件流式写入本地，任务曾运行过时按已落盘字节续传
async fn stream_download(
    sftp: &SftpSession,
    task: &Arc<TaskState>,
    remote_path: &str,
    generation: u64,
) -> Result<bool> {
    let Some(metadata) = await_result_with_control(
        task,
        generation,
        SFTP_REQUEST_TIMEOUT,
        sftp.metadata(remote_path),
        "读取远端文件信息超时",
        |error| anyhow!("读取远端文件信息失败：{}", format_sftp_error(&error)),
    )
    .await?
    else {
        return Ok(false);
    };
    let total = metadata.size.unwrap_or(0);
    task.total.store(total, Ordering::SeqCst);

    if !check_control(task, generation) {
        return Ok(false);
    }
    if let Some(parent) = Path::new(&task.local_path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| anyhow!("创建本地目录失败：{}", error))?;
    }

    // 断点定位：按本地已落盘字节续传
    let mut offset = 0u64;
    if task.started_once.load(Ordering::SeqCst) {
        if let Ok(metadata) = tokio::fs::metadata(&task.local_path).await {
            if metadata.len() <= total {
                offset = metadata.len();
            }
        }
    }
    task.started_once.store(true, Ordering::SeqCst);
    if total > 0 && offset >= total {
        task.transferred.store(total, Ordering::SeqCst);
        return Ok(true);
    }

    let Some(mut remote) = await_result_with_control(
        task,
        generation,
        SFTP_REQUEST_TIMEOUT,
        sftp.open_with_flags(remote_path, OpenFlags::READ),
        "打开远端文件超时",
        |error| anyhow!("打开远端文件失败：{}", format_sftp_error(&error)),
    )
    .await?
    else {
        return Ok(false);
    };
    // 不截断打开以支持续传，首次运行由 set_len(0) 显式清空
    let mut local = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&task.local_path)
        .await
        .map_err(|error| anyhow!("打开本地文件失败：{}", error))?;
    if offset > 0 {
        let Some(_) = await_result_with_control(
            task,
            generation,
            SFTP_REQUEST_TIMEOUT,
            remote.seek(std::io::SeekFrom::Start(offset)),
            "定位远端文件超时",
            |error| anyhow!("定位远端文件失败：{}", error),
        )
        .await?
        else {
            let _ = timeout(SFTP_CLOSE_TIMEOUT, remote.shutdown()).await;
            return Ok(false);
        };
        local
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|error| anyhow!("定位本地文件失败：{}", error))?;
    } else {
        local
            .set_len(0)
            .await
            .map_err(|error| anyhow!("清空本地文件失败：{}", error))?;
    }
    task.transferred.store(offset, Ordering::SeqCst);

    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        if !check_control(task, generation) {
            let _ = timeout(SFTP_CLOSE_TIMEOUT, local.flush()).await;
            let _ = timeout(SFTP_CLOSE_TIMEOUT, remote.shutdown()).await;
            return Ok(false);
        }
        let Some(n) = await_result_with_control(
            task,
            generation,
            SFTP_REQUEST_TIMEOUT,
            remote.read(&mut buf),
            "读取远端文件超时",
            |error| anyhow!("读取远端文件失败：{}", error),
        )
        .await?
        else {
            let _ = timeout(SFTP_CLOSE_TIMEOUT, local.flush()).await;
            let _ = timeout(SFTP_CLOSE_TIMEOUT, remote.shutdown()).await;
            return Ok(false);
        };
        if n == 0 {
            break;
        }
        local
            .write_all(&buf[..n])
            .await
            .map_err(|error| anyhow!("写入本地文件失败：{}", error))?;
        task.transferred.fetch_add(n as u64, Ordering::SeqCst);
    }
    local
        .flush()
        .await
        .map_err(|error| anyhow!("刷新本地文件失败：{}", error))?;
    let _ = timeout(SFTP_CLOSE_TIMEOUT, remote.shutdown()).await;
    // 远端提前收到 EOF 说明连接异常中断，交给重试按断点续传
    if task.transferred.load(Ordering::SeqCst) < total {
        return Err(anyhow!("传输中断，数据不完整"));
    }
    Ok(true)
}

/// 执行一次打包下载：远端 tar 打包 -> 下载压缩包 -> 清理远端临时包
///
/// 已完成的临时包大小与记录一致时跳过打包，暂停会在打包完成后阻止下载；
/// 每次打包尝试使用独立路径，确认成功后才登记为下载文件，避免断线后的旧进程污染重试结果。
async fn run_archive_once(app: &AppHandle, task: &Arc<TaskState>, generation: u64) -> Result<bool> {
    let archive = task
        .archive
        .as_ref()
        .ok_or_else(|| anyhow!("打包任务信息缺失"))?;
    let job = archive.job();
    let manager = app.state::<SessionManager>();
    let Some(sftp) = sftp_with_control(&manager, task, generation).await? else {
        return Ok(false);
    };

    // 已验证归档以路径和大小整体发布，started_once 仅负责本地文件断点。
    let existing_archive = archive.ready();
    let packed = if let Some(ready_archive) = existing_archive.as_ref() {
        match await_with_control(
            task,
            generation,
            ARCHIVE_METADATA_TIMEOUT,
            sftp.metadata(&ready_archive.path),
        )
        .await
        {
            None => {
                let _ = check_control(task, generation);
                return Ok(false);
            }
            Some(Ok(Ok(metadata))) => metadata.size == Some(ready_archive.size),
            Some(Ok(Err(_))) | Some(Err(_)) => false,
        }
    } else {
        false
    };

    let download_path = if packed {
        let ready_archive = existing_archive.ok_or_else(|| anyhow!("远端打包结果状态缺失"))?;
        task.total.store(ready_archive.size, Ordering::SeqCst);
        ready_archive.path
    } else {
        if let Some(stale_archive) = archive.take_ready() {
            cleanup_remote_tmp(app, &task.session_id, &stale_archive.path).await;
        }
        // 远端打包尚未开始时，暂停请求可以立即落地。
        if !check_control(task, generation) {
            return Ok(false);
        }
        if !task.control.begin_packing(generation) {
            return Ok(false);
        }
        task.transferred.store(0, Ordering::SeqCst);
        // 重新打包后旧的本地半成品作废，禁止续传
        task.started_once.store(false, Ordering::SeqCst);
        let names = job
            .names
            .iter()
            .map(|name| shell_quote(name))
            .collect::<Vec<_>>()
            .join(" ");
        let attempt_tmp = format!("/tmp/ztshell-{}.tar.gz", uuid::Uuid::new_v4());
        let command = format!(
            "cd {} && tar -czf {} -- {} >/dev/null 2>&1 && printf __ZTOK__ || {{ rm -f -- {}; printf __ZTFAIL__; }}",
            shell_quote(&job.remote_dir),
            shell_quote(&attempt_tmp),
            names,
            shell_quote(&attempt_tmp)
        );
        let operation_id = format!("transfer-pack:{}", task.id);
        let mut operation = manager.begin_operation(&task.session_id, &operation_id)?;
        // 注册中断入口后重新检查，覆盖删除或暂停恰好发生在注册前的竞态。
        if !check_control(task, generation) {
            return Ok(false);
        }
        let output = match manager
            .exec_cancellable(&task.session_id, &command, operation.cancellation())
            .await
        {
            Ok(output) => output,
            Err(error) => {
                drop(operation);
                cleanup_remote_tmp(app, &task.session_id, &attempt_tmp).await;
                if !check_control(task, generation) {
                    return Ok(false);
                }
                return Err(error);
            }
        };
        drop(operation);
        if !task.control.is_generation(generation) {
            cleanup_remote_tmp(app, &task.session_id, &attempt_tmp).await;
            return Ok(false);
        }
        if !output.contains("__ZTOK__") {
            cleanup_remote_tmp(app, &task.session_id, &attempt_tmp).await;
            if !check_control(task, generation) {
                return Ok(false);
            }
            return Err(anyhow!("远端打包失败，请检查文件权限"));
        }
        let packed_metadata = tokio::select! {
            biased;
            _ = wait_for_task_cancellation(task, generation) => None,
            result = timeout(ARCHIVE_METADATA_TIMEOUT, sftp.metadata(&attempt_tmp)) => {
                Some(result)
            }
        };
        let packed_total = match packed_metadata {
            None => {
                cleanup_remote_tmp(app, &task.session_id, &attempt_tmp).await;
                return Ok(false);
            }
            Some(Ok(Ok(metadata))) => metadata.size.unwrap_or(0),
            Some(Ok(Err(error))) => {
                cleanup_remote_tmp(app, &task.session_id, &attempt_tmp).await;
                if !check_control(task, generation) {
                    return Ok(false);
                }
                return Err(anyhow!(
                    "读取远端打包结果失败：{}",
                    format_sftp_error(&error)
                ));
            }
            Some(Err(_)) => {
                cleanup_remote_tmp(app, &task.session_id, &attempt_tmp).await;
                if !check_control(task, generation) {
                    return Ok(false);
                }
                return Err(anyhow!("读取远端打包结果超时"));
            }
        };
        if !task.control.is_generation(generation) {
            cleanup_remote_tmp(app, &task.session_id, &attempt_tmp).await;
            return Ok(false);
        }

        // 只有当前执行代际可以发布下载路径；删除竞态由后续状态检查负责回收。
        archive.publish(attempt_tmp.clone(), packed_total);
        task.total.store(packed_total, Ordering::SeqCst);
        // 暂停只阻止后续下载，完整的远端包留待继续时复用。
        if !check_control(task, generation) {
            if task.status() == ST_CANCELLED {
                if let Some(ready_archive) = archive.take_ready() {
                    cleanup_remote_tmp(app, &task.session_id, &ready_archive.path).await;
                }
            }
            return Ok(false);
        }
        if !task.control.finish_packing(generation) {
            return Ok(false);
        }
        attempt_tmp
    };

    if !check_control(task, generation) {
        return Ok(false);
    }
    let finished = stream_download(&sftp, task, &download_path, generation).await?;
    if finished && task.control.is_generation(generation) {
        // 成功取得归档所有权的一方负责清理；删除可能已经先一步接管。
        if let Some(ready_archive) = archive.take_ready() {
            cleanup_remote_tmp(app, &task.session_id, &ready_archive.path).await;
        }
    }
    Ok(finished)
}

/// 清理远端临时压缩包（失败忽略，/tmp 会由系统回收）
async fn cleanup_remote_tmp(app: &AppHandle, session_id: &str, remote_tmp: &str) {
    let manager = app.state::<SessionManager>();
    let command = format!("rm -f -- {}", shell_quote(remote_tmp));
    let _ = timeout(REMOTE_CLEANUP_TIMEOUT, manager.exec(session_id, &command)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;
    use tokio::fs;
    use uuid::Uuid;

    /// 同名文件必须确认覆盖，同名文件夹不可覆盖，检查过程不得改动已有内容。
    #[tokio::test]
    async fn pack_download_checks_conflicts_without_changing_local_entries() {
        let directory = temp_dir().join(format!("ztshell-download-{}", Uuid::new_v4()));
        fs::create_dir(&directory).await.unwrap();
        let target = directory.join("archive.tar.gz");
        assert!(!pack_download_needs_overwrite(&target, false).await.unwrap());
        assert!(!target.exists());

        fs::write(&target, b"existing archive").await.unwrap();
        assert!(pack_download_needs_overwrite(&target, false).await.unwrap());
        assert!(!pack_download_needs_overwrite(&target, true).await.unwrap());
        assert_eq!(fs::read(&target).await.unwrap(), b"existing archive");

        fs::remove_file(&target).await.unwrap();
        fs::create_dir(&target).await.unwrap();
        let child = target.join("keep.txt");
        fs::write(&child, b"keep").await.unwrap();
        for overwrite in [false, true] {
            let error = pack_download_needs_overwrite(&target, overwrite)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("同名文件夹"));
        }
        assert_eq!(fs::read(&child).await.unwrap(), b"keep");
        fs::remove_dir_all(directory).await.unwrap();
    }

    /// 下载目录缺失或变成文件时，即使已确认覆盖也不能放行。
    #[tokio::test]
    async fn pack_download_rejects_invalid_download_directory() {
        let directory = temp_dir().join(format!("ztshell-download-{}", Uuid::new_v4()));
        let target = directory.join("archive.tar.gz");
        for overwrite in [false, true] {
            assert!(pack_download_needs_overwrite(&target, overwrite)
                .await
                .is_err());
        }
        assert!(!directory.exists());

        fs::write(&directory, b"not a directory").await.unwrap();
        for overwrite in [false, true] {
            assert!(pack_download_needs_overwrite(&target, overwrite)
                .await
                .is_err());
        }
        assert_eq!(fs::read(&directory).await.unwrap(), b"not a directory");
        fs::remove_file(directory).await.unwrap();
    }

    /// 构造不依赖 Tauri 运行时的最小任务对象。
    fn task_with_status(status: u8) -> TaskState {
        TaskState {
            id: "task-test".to_string(),
            parent_id: None,
            session_id: "session-test".to_string(),
            kind: TransferKind::Upload,
            is_dir: false,
            name: "test.bin".to_string(),
            local_path: "C:/test.bin".to_string(),
            remote_path: "/tmp/test.bin".to_string(),
            archive: None,
            control: TaskControl::new(status),
            transferred: AtomicU64::new(0),
            total: AtomicU64::new(10),
            speed: AtomicU64::new(1),
            eta_secs: AtomicI64::new(9),
            elapsed_ms: AtomicU64::new(100),
            error: Mutex::new(String::new()),
            started_once: AtomicBool::new(true),
            runner_lock: AsyncMutex::new(()),
        }
    }

    /// 每次采样按本周期字节增量计算速度，中断和重试不继承旧速度。
    #[test]
    fn progress_speed_uses_sampled_bytes() {
        let manager = TransferManager::default();
        let task = Arc::new(task_with_status(ST_RUNNING));
        task.total.store(1000, Ordering::SeqCst);
        manager.register(task.clone());

        task.transferred.store(256, Ordering::SeqCst);
        assert_eq!(manager.collect_progress(1000)[0].speed, 256);
        task.transferred.store(384, Ordering::SeqCst);
        assert_eq!(manager.collect_progress(1000)[0].speed, 128);
        assert_eq!(manager.collect_progress(1000)[0].speed, 0);

        assert!(task.control.interrupt());
        assert_eq!(manager.collect_progress(1000)[0].speed, 0);
        assert!(task.control.retry_failed());
        assert!(task.control.start(task.control.generation()));
        task.transferred.store(584, Ordering::SeqCst);
        assert_eq!(manager.collect_progress(1000)[0].speed, 200);
    }

    /// 网络断开会把活动任务转为可重试失败，并使旧执行代际失效。
    #[test]
    fn interrupts_active_task() {
        let task = task_with_status(ST_RUNNING);

        assert!(mark_task_interrupted(&task));
        assert_eq!(task.status(), ST_FAILED);
        assert_eq!(task.control.generation(), 1);
        assert!(task.control.cancelled(0));
        assert_eq!(
            task.error.lock().unwrap().as_str(),
            SESSION_INTERRUPTED_MESSAGE
        );
        assert_eq!(task.speed.load(Ordering::SeqCst), 0);
        assert_eq!(task.eta_secs.load(Ordering::SeqCst), -1);
    }

    /// 已暂停、失败、完成或取消的任务不应被重复标记为会话中断。
    #[test]
    fn does_not_change_inactive_task() {
        for status in [ST_PAUSED, ST_FAILED, ST_COMPLETED, ST_CANCELLED] {
            let task = task_with_status(status);

            assert!(!mark_task_interrupted(&task));
            assert_eq!(task.status(), status);
            assert_eq!(task.control.generation(), 0);
        }
    }

    /// 等待中的任务收到暂停请求后应立即进入暂停状态。
    #[test]
    fn pauses_pending_task() {
        let task = task_with_status(ST_PENDING);

        request_pause(&task);

        assert_eq!(task.status(), ST_PAUSED);
    }

    /// 运行中的任务收到暂停请求时应唤醒正在等待的异步操作。
    #[tokio::test]
    async fn pause_notifies_waiting_runner() {
        let task = Arc::new(task_with_status(ST_RUNNING));
        let waiting_task = task.clone();
        let waiter = tokio::spawn(async move {
            wait_for_task_control(&waiting_task, 0).await;
        });
        tokio::task::yield_now().await;

        request_pause(&task);

        assert!(timeout(Duration::from_secs(1), waiter).await.is_ok());
        assert!(!check_control(&task, 0));
        assert_eq!(task.status(), ST_PAUSED);
    }

    /// 打包完成后的暂停只改变任务状态，不应使已完成的远端包失效。
    #[test]
    fn pause_after_packing_keeps_archive_ready() {
        let mut task = task_with_status(ST_PACKING);
        task.archive = Some(ArchiveRuntime::new(
            "/remote".to_string(),
            vec!["file".to_string()],
        ));
        let archive = task.archive.as_ref().unwrap();
        archive.publish("/tmp/archive-test.tar.gz".to_string(), 128);
        request_pause(&task);

        assert!(!check_control(&task, 0));
        assert_eq!(task.status(), ST_PAUSED);
        assert_eq!(archive.ready().unwrap().path, "/tmp/archive-test.tar.gz");
    }

    /// 打包阶段达到重试上限时也必须进入失败状态，不能卡在打包中。
    #[test]
    fn marks_packing_runner_failed() {
        let task = task_with_status(ST_PACKING);

        assert!(mark_runner_failed(&task, 0, "测试失败"));
        assert_eq!(task.status(), ST_FAILED);
        assert_eq!(task.error.lock().unwrap().as_str(), "测试失败");
    }

    /// 旧执行代际不能把错误写回已经重新入队的新执行体。
    #[test]
    fn stale_runner_cannot_mark_new_generation_failed() {
        let task = task_with_status(ST_RUNNING);
        task.control.set_generation(1);

        assert!(!mark_runner_failed(&task, 0, "旧连接错误"));
        assert_eq!(task.status(), ST_RUNNING);
        assert!(task.error.lock().unwrap().is_empty());
    }
}
