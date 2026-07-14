//! 传输任务管理器：上传/下载任务队列、断点续传、暂停继续、打包下载与进度事件推送
//!
//! 设计要点：
//! - 任务组织为树：文件夹任务是聚合节点（进度/状态由子任务汇总），文件任务是实际执行单元
//! - 并发由全局信号量限制，同一时刻最多 3 个文件在传输，其余任务排队等待
//! - 断点续传：任务曾经运行过（started_once）时按已落盘字节 seek 续传，首次运行覆盖写
//! - 网络波动自动重试 3 次，超过后标记失败，可通过"重试失败的作业"手动续传
//! - 进度由后台定时循环节流推送：结构变化推 transfer://changed 全量，动态变化推 transfer://progress 增量

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use dashmap::{DashMap, DashSet};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Semaphore;

use super::manager::SessionManager;
use super::sftp::format_sftp_error;

/// 任务状态：等待中
const ST_PENDING: u8 = 0;
/// 任务状态：传输中
const ST_RUNNING: u8 = 1;
/// 任务状态：远端打包中（仅打包下载任务）
const ST_PACKING: u8 = 2;
/// 任务状态：已暂停
const ST_PAUSED: u8 = 3;
/// 任务状态：已失败
const ST_FAILED: u8 = 4;
/// 任务状态：已完成
const ST_COMPLETED: u8 = 5;
/// 任务状态：已取消（删除任务时置位，用于让排队/运行中的执行体退出）
const ST_CANCELLED: u8 = 6;

/// 控制指令：无
const CTL_NONE: u8 = 0;
/// 控制指令：请求暂停
const CTL_PAUSE: u8 = 1;
/// 控制指令：请求取消
const CTL_CANCEL: u8 = 2;

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
const TICK_MS: u64 = 300;

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

/// 打包下载的附加信息
#[derive(Debug, Clone)]
pub struct ArchiveJob {
    /// 打包目标所在的远端目录
    pub remote_dir: String,
    /// 参与打包的条目名称列表
    pub names: Vec<String>,
    /// 远端临时压缩包路径
    pub remote_tmp: String,
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
    pub archive: Option<ArchiveJob>,
    /// 当前状态
    pub status: AtomicU8,
    /// 控制指令（暂停/取消请求）
    pub control: AtomicU8,
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
}

impl TaskState {
    /// 读取当前状态
    fn status(&self) -> u8 {
        self.status.load(Ordering::SeqCst)
    }

    /// 状态转前端字符串
    fn status_str(&self) -> &'static str {
        status_str(self.status())
    }
}

/// 状态码转前端字符串
fn status_str(status: u8) -> &'static str {
    match status {
        ST_RUNNING => "running",
        ST_PACKING => "packing",
        ST_PAUSED => "paused",
        ST_FAILED => "failed",
        ST_COMPLETED => "completed",
        ST_CANCELLED => "cancelled",
        _ => "pending",
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
        Self { need_confirm: false, file_count, active_count: 0, exist_names: Vec::new() }
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
    /// 进度循环的速度跟踪（上次字节数与平滑速度）
    speed_track: Mutex<HashMap<String, (u64, f64)>>,
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

/// 单引号转义 shell 参数，防止特殊字符破坏命令
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl TransferManager {
    /// 注册一个任务并纳入展示顺序
    fn register(&self, task: Arc<TaskState>) {
        if let Some(parent) = task.parent_id.clone() {
            self.children.entry(parent).or_default().push(task.id.clone());
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
        archive: Option<ArchiveJob>,
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
            status: AtomicU8::new(ST_PENDING),
            control: AtomicU8::new(CTL_NONE),
            transferred: AtomicU64::new(0),
            total: AtomicU64::new(total),
            speed: AtomicU64::new(0),
            eta_secs: AtomicI64::new(-1),
            elapsed_ms: AtomicU64::new(0),
            error: Mutex::new(String::new()),
            started_once: AtomicBool::new(false),
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
                file_count, MAX_TOTAL_FILES
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
                let parent_id = dir_ids.get(&parent_rel).cloned().unwrap_or_else(|| root_id.clone());
                let name = entry.rel.rsplit('/').next().unwrap_or(&entry.rel).to_string();
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
                if self.children.get(id).map(|c| !c.is_empty()).unwrap_or(false) {
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
                let parent_id = dir_ids.get(&parent_rel).cloned().unwrap_or_else(|| root_id.clone());
                let entry_name = entry.rel.rsplit('/').next().unwrap_or(&entry.rel).to_string();
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
                if self.children.get(id).map(|c| !c.is_empty()).unwrap_or(false) {
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
    ) -> Result<()> {
        if names.is_empty() {
            return Err(anyhow!("未选择需要打包的文件"));
        }
        let manager = app.state::<SessionManager>();
        // 先探测远端 tar 命令是否可用，失败时立刻反馈
        let probe = manager
            .exec(session_id, "command -v tar >/dev/null 2>&1 && printf __ZTOK__ || printf __ZTNO__")
            .await?;
        if !probe.contains("__ZTOK__") {
            return Err(anyhow!("远端未找到 tar 命令，无法打包下载"));
        }
        let remote_tmp = format!("/tmp/ztshell-{}.tar.gz", uuid::Uuid::new_v4());
        let file_name = Path::new(&local_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "archive.tar.gz".to_string());
        let task = self.new_task(
            session_id,
            None,
            TransferKind::Download,
            false,
            file_name,
            local_path,
            remote_dir.clone(),
            0,
            Some(ArchiveJob { remote_dir, names, remote_tmp }),
        );
        self.register(task.clone());
        spawn_file_runner(app.clone(), task);
        self.emit_changed(app);
        Ok(())
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
            match task.status() {
                ST_PENDING => task.status.store(ST_PAUSED, Ordering::SeqCst),
                ST_RUNNING | ST_PACKING => task.control.store(CTL_PAUSE, Ordering::SeqCst),
                _ => {}
            }
        }
        self.emit_changed(app);
    }

    /// 继续任务：已暂停的重新入队；暂停请求尚未落地的撤销请求
    pub fn resume(&self, app: &AppHandle, ids: Option<Vec<String>>) {
        for task in self.collect_targets(ids, false) {
            // 撤销尚未落地的暂停请求
            let _ = task.control.compare_exchange(
                CTL_PAUSE,
                CTL_NONE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            if task.status() == ST_PAUSED {
                task.status.store(ST_PENDING, Ordering::SeqCst);
                task.error.lock().unwrap().clear();
                if task.is_dir {
                    spawn_dir_creator(app.clone(), task.clone());
                } else {
                    spawn_file_runner(app.clone(), task.clone());
                }
            }
        }
        self.emit_changed(app);
    }

    /// 删除任务（级联子树）：取消执行并从列表移除，打包任务附带清理远端临时包
    pub fn remove(&self, app: &AppHandle, ids: Option<Vec<String>>) {
        let targets = self.collect_targets(ids, true);
        let removed: HashSet<String> = targets.iter().map(|t| t.id.clone()).collect();
        for task in &targets {
            task.control.store(CTL_CANCEL, Ordering::SeqCst);
            task.status.store(ST_CANCELLED, Ordering::SeqCst);
            self.tasks.remove(&task.id);
            self.children.remove(&task.id);
            // 打包任务删除时清理远端临时压缩包
            if let Some(job) = task.archive.clone() {
                let app = app.clone();
                let session_id = task.session_id.clone();
                tauri::async_runtime::spawn(async move {
                    cleanup_remote_tmp(&app, &session_id, &job.remote_tmp).await;
                });
            }
        }
        // 维护展示顺序与父子索引
        self.order.lock().unwrap().retain(|id| !removed.contains(id));
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

    /// 重试失败的任务（断点续传接续已传部分），session_id 为空时重试全部会话
    pub fn retry_failed(&self, app: &AppHandle, session_id: Option<&str>) {
        for task in self.collect_targets(None, false) {
            if task.status() != ST_FAILED {
                continue;
            }
            if let Some(sid) = session_id {
                if task.session_id != sid {
                    continue;
                }
            }
            task.error.lock().unwrap().clear();
            task.status.store(ST_PENDING, Ordering::SeqCst);
            task.control.store(CTL_NONE, Ordering::SeqCst);
            if task.is_dir {
                spawn_dir_creator(app.clone(), task.clone());
            } else {
                spawn_file_runner(app.clone(), task.clone());
            }
        }
        self.emit_changed(app);
    }

    /// 移除指定会话的全部传输任务（会话断开时调用，避免遗留不可见的僵尸任务）
    pub fn remove_session(&self, app: &AppHandle, session_id: &str) {
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

        // 第一步：计算执行单元（非聚合节点）的瞬时速度（指数平滑）
        {
            let mut track = self.speed_track.lock().unwrap();
            for id in &order {
                let Some(task) = self.tasks.get(id) else { continue };
                let has_children = self.children.get(id).map(|c| !c.is_empty()).unwrap_or(false);
                if task.is_dir && has_children {
                    continue;
                }
                let cur = task.transferred.load(Ordering::SeqCst);
                let entry = track.entry(id.clone()).or_insert((cur, 0.0));
                let instant_speed = (cur.saturating_sub(entry.0)) as f64 / dt_secs;
                let ema = if task.status() == ST_RUNNING {
                    instant_speed * 0.5 + entry.1 * 0.5
                } else {
                    0.0
                };
                *entry = (cur, ema);
                task.speed.store(ema as u64, Ordering::SeqCst);
            }
        }

        // 第二步：自底向上聚合目录节点（order 保证父先于子，反向遍历即自底向上）
        for id in order.iter().rev() {
            let Some(task) = self.tasks.get(id).map(|t| t.clone()) else { continue };
            let kids = self.children.get(id).map(|c| c.clone()).unwrap_or_default();
            if !task.is_dir || kids.is_empty() {
                continue;
            }
            let mut transferred = 0u64;
            let mut total = 0u64;
            let mut speed = 0u64;
            let mut has = [false; 7];
            for kid in &kids {
                let Some(child) = self.tasks.get(kid) else { continue };
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
            task.status.store(status, Ordering::SeqCst);
        }

        // 第三步：累计耗时、计算预计剩余，并与上次快照比对生成增量
        let mut updates = Vec::new();
        let mut snapshot = self.snapshot.lock().unwrap();
        for id in &order {
            let Some(task) = self.tasks.get(id) else { continue };
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
fn scan_local_roots(paths: &[String]) -> Result<Vec<(PathBuf, String, bool, Option<Vec<ScanEntry>>)>> {
    let mut out = Vec::new();
    for raw in paths {
        let path = PathBuf::from(raw);
        let meta = std::fs::metadata(&path).map_err(|e| anyhow!("读取本地路径失败（{}）：{}", raw, e))?;
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
            let file_type = entry.file_type().map_err(|e| anyhow!("读取文件类型失败：{}", e))?;
            if file_type.is_dir() {
                out.push(ScanEntry { rel: rel_str, is_dir: true, size: 0 });
                stack.push(child_rel);
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(ScanEntry { rel: rel_str, is_dir: false, size });
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
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
            let meta = item.metadata();
            if matches!(meta.file_type(), russh_sftp::protocol::FileType::Dir) {
                out.push(ScanEntry { rel: child_rel.clone(), is_dir: true, size: 0 });
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
                return Err(anyhow!("创建远端目录失败（{}）：{}", prefix, format_sftp_error(&e)));
            }
        }
        tm.dir_cache.insert(key);
    }
    Ok(())
}

/// 检查控制指令：收到暂停请求时落地为暂停状态；返回 false 表示应停止传输
fn check_control(task: &TaskState) -> bool {
    match task.control.load(Ordering::SeqCst) {
        CTL_PAUSE => {
            task.status.store(ST_PAUSED, Ordering::SeqCst);
            task.control.store(CTL_NONE, Ordering::SeqCst);
            false
        }
        CTL_CANCEL => false,
        _ => task.status() != ST_CANCELLED,
    }
}

/// 可中断的重试等待：期间响应暂停/取消，返回 false 表示应终止
async fn sleep_with_control(task: &TaskState, ms: u64) -> bool {
    let steps = ms / 100;
    for _ in 0..steps {
        if !check_control(task) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    check_control(task)
}

/// 启动空目录任务：上传方向创建远端目录，下载方向创建本地目录
fn spawn_dir_creator(app: AppHandle, task: Arc<TaskState>) {
    tauri::async_runtime::spawn(async move {
        if task
            .status
            .compare_exchange(ST_PENDING, ST_RUNNING, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
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
                    .map_err(|e| anyhow!("创建本地目录失败：{}", e)),
            }
        }
        .await;
        match result {
            Ok(()) => task.status.store(ST_COMPLETED, Ordering::SeqCst),
            Err(e) => {
                *task.error.lock().unwrap() = e.to_string();
                task.status.store(ST_FAILED, Ordering::SeqCst);
            }
        }
    });
}

/// 启动文件传输任务执行体：受并发信号量约束，失败自动重试
fn spawn_file_runner(app: AppHandle, task: Arc<TaskState>) {
    tauri::async_runtime::spawn(async move {
        let semaphore = app.state::<TransferManager>().semaphore.clone();
        let Ok(_permit) = semaphore.acquire_owned().await else {
            return;
        };
        // 排队期间可能被暂停或删除；CAS 置为运行中，防止快速暂停/继续时多个执行体同时进入
        if task
            .status
            .compare_exchange(ST_PENDING, ST_RUNNING, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        task.error.lock().unwrap().clear();
        let mut attempt: u32 = 0;
        loop {
            let result = if task.archive.is_some() {
                run_archive_once(&app, &task).await
            } else {
                match task.kind {
                    TransferKind::Upload => run_upload_once(&app, &task).await,
                    TransferKind::Download => run_download_once(&app, &task).await,
                }
            };
            match result {
                Ok(true) => {
                    task.status.store(ST_COMPLETED, Ordering::SeqCst);
                    break;
                }
                // 被暂停或取消，状态已在检查点落地
                Ok(false) => break,
                Err(e) => {
                    attempt += 1;
                    if attempt >= MAX_ATTEMPTS {
                        *task.error.lock().unwrap() = e.to_string();
                        task.status.store(ST_FAILED, Ordering::SeqCst);
                        break;
                    }
                    // 网络波动自动重试，重试后按已落盘字节续传
                    if !sleep_with_control(&task, RETRY_DELAY_MS).await {
                        break;
                    }
                    task.status.store(ST_RUNNING, Ordering::SeqCst);
                }
            }
        }
    });
}

/// 执行一次上传：返回 Ok(true) 完成、Ok(false) 被暂停/取消、Err 出错待重试
async fn run_upload_once(app: &AppHandle, task: &Arc<TaskState>) -> Result<bool> {
    let manager = app.state::<SessionManager>();
    let sftp = manager.sftp(&task.session_id).await?;
    let tm = app.state::<TransferManager>();
    ensure_remote_dir(&tm, &sftp, &task.session_id, &remote_parent(&task.remote_path)).await?;

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
        if !check_control(task) {
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
    remote.flush().await.map_err(|e| anyhow!("刷新远端文件失败：{}", e))?;
    let _ = remote.shutdown().await;
    Ok(true)
}

/// 执行一次下载：返回含义同上传
async fn run_download_once(app: &AppHandle, task: &Arc<TaskState>) -> Result<bool> {
    let manager = app.state::<SessionManager>();
    let sftp = manager.sftp(&task.session_id).await?;
    stream_download(&sftp, task, &task.remote_path.clone()).await
}

/// 下载核心：远端文件流式写入本地，任务曾运行过时按本地已落盘字节续传
async fn stream_download(sftp: &SftpSession, task: &Arc<TaskState>, remote_path: &str) -> Result<bool> {
    let total = sftp
        .metadata(remote_path)
        .await
        .map_err(|e| anyhow!("读取远端文件信息失败：{}", format_sftp_error(&e)))?
        .size
        .unwrap_or(0);
    task.total.store(total, Ordering::SeqCst);

    if let Some(parent) = Path::new(&task.local_path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow!("创建本地目录失败：{}", e))?;
    }

    // 断点定位：按本地已落盘字节续传
    let mut offset = 0u64;
    if task.started_once.load(Ordering::SeqCst) {
        if let Ok(meta) = tokio::fs::metadata(&task.local_path).await {
            if meta.len() <= total {
                offset = meta.len();
            }
        }
    }
    task.started_once.store(true, Ordering::SeqCst);
    if total > 0 && offset >= total {
        task.transferred.store(total, Ordering::SeqCst);
        return Ok(true);
    }

    let mut remote = sftp
        .open_with_flags(remote_path, OpenFlags::READ)
        .await
        .map_err(|e| anyhow!("打开远端文件失败：{}", format_sftp_error(&e)))?;
    // 不截断打开以支持续传，首次运行由 set_len(0) 显式清空
    let mut local = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&task.local_path)
        .await
        .map_err(|e| anyhow!("打开本地文件失败：{}", e))?;
    if offset > 0 {
        remote
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| anyhow!("定位远端文件失败：{}", e))?;
        local
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| anyhow!("定位本地文件失败：{}", e))?;
    } else {
        local
            .set_len(0)
            .await
            .map_err(|e| anyhow!("清空本地文件失败：{}", e))?;
    }
    task.transferred.store(offset, Ordering::SeqCst);

    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        if !check_control(task) {
            let _ = local.flush().await;
            let _ = remote.shutdown().await;
            return Ok(false);
        }
        let n = remote
            .read(&mut buf)
            .await
            .map_err(|e| anyhow!("读取远端文件失败：{}", e))?;
        if n == 0 {
            break;
        }
        local
            .write_all(&buf[..n])
            .await
            .map_err(|e| anyhow!("写入本地文件失败：{}", e))?;
        task.transferred.fetch_add(n as u64, Ordering::SeqCst);
    }
    local.flush().await.map_err(|e| anyhow!("刷新本地文件失败：{}", e))?;
    let _ = remote.shutdown().await;
    // 远端提前收到 EOF 说明连接异常中断，交给重试按断点续传
    if task.transferred.load(Ordering::SeqCst) < total {
        return Err(anyhow!("传输中断，数据不完整"));
    }
    Ok(true)
}

/// 执行一次打包下载：远端 tar 打包 -> 下载压缩包 -> 清理远端临时包
///
/// 上次打包成功但下载中断时（临时包大小与记录一致），跳过打包直接续传下载；
/// 打包阶段的 exec 不可中断，暂停/取消会在打包结束后的检查点落地
async fn run_archive_once(app: &AppHandle, task: &Arc<TaskState>) -> Result<bool> {
    let job = task
        .archive
        .clone()
        .ok_or_else(|| anyhow!("打包任务信息缺失"))?;
    let manager = app.state::<SessionManager>();
    let sftp = manager.sftp(&task.session_id).await?;

    // 判断是否可以复用上次打好的压缩包续传下载
    let mut packed = false;
    let recorded_total = task.total.load(Ordering::SeqCst);
    if task.started_once.load(Ordering::SeqCst) && recorded_total > 0 {
        if let Ok(meta) = sftp.metadata(&job.remote_tmp).await {
            if meta.size.unwrap_or(0) == recorded_total {
                packed = true;
            }
        }
    }

    if !packed {
        task.status.store(ST_PACKING, Ordering::SeqCst);
        task.transferred.store(0, Ordering::SeqCst);
        // 重新打包后旧的本地半成品作废，禁止续传
        task.started_once.store(false, Ordering::SeqCst);
        let names = job
            .names
            .iter()
            .map(|n| shell_quote(n))
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "cd {} && tar -czf {} -- {} >/dev/null 2>&1 && printf __ZTOK__ || printf __ZTFAIL__",
            shell_quote(&job.remote_dir),
            shell_quote(&job.remote_tmp),
            names
        );
        let output = manager.exec(&task.session_id, &command).await?;
        if !check_control(task) {
            cleanup_remote_tmp(app, &task.session_id, &job.remote_tmp).await;
            return Ok(false);
        }
        if !output.contains("__ZTOK__") {
            return Err(anyhow!("远端打包失败，请检查文件权限"));
        }
        task.status.store(ST_RUNNING, Ordering::SeqCst);
    }

    let finished = stream_download(&sftp, task, &job.remote_tmp).await?;
    if finished {
        // 下载完成后清理远端临时压缩包
        cleanup_remote_tmp(app, &task.session_id, &job.remote_tmp).await;
    }
    Ok(finished)
}

/// 清理远端临时压缩包（失败忽略，/tmp 会由系统回收）
async fn cleanup_remote_tmp(app: &AppHandle, session_id: &str, remote_tmp: &str) {
    let manager = app.state::<SessionManager>();
    let command = format!("rm -f {}", shell_quote(remote_tmp));
    let _ = manager.exec(session_id, &command).await;
}
