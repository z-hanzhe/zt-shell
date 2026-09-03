//! 传输任务控制状态：串行化暂停、继续、取消、重试与执行代际转换。

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;

use tokio::sync::watch;

/// 任务状态：等待中
pub(super) const ST_PENDING: u8 = 0;
/// 任务状态：传输中
pub(super) const ST_RUNNING: u8 = 1;
/// 任务状态：远端打包中
pub(super) const ST_PACKING: u8 = 2;
/// 任务状态：已暂停
pub(super) const ST_PAUSED: u8 = 3;
/// 任务状态：已失败
pub(super) const ST_FAILED: u8 = 4;
/// 任务状态：已完成
pub(super) const ST_COMPLETED: u8 = 5;
/// 任务状态：已取消
pub(super) const ST_CANCELLED: u8 = 6;

/// 控制指令：无
const CTL_NONE: u8 = 0;
/// 控制指令：请求暂停
const CTL_PAUSE: u8 = 1;
/// 控制指令：请求取消
const CTL_CANCEL: u8 = 2;

/// 将状态码转换为前端使用的名称。
pub(super) fn status_str(status: u8) -> &'static str {
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

/// 继续操作的结果。
pub(super) enum ResumeAction {
    /// 当前执行体继续运行，无需创建新执行体。
    KeepRunning,
    /// 已暂停任务进入新执行代际，需要重新入队。
    Requeue,
    /// 当前状态不支持继续。
    Ignored,
}

/// 单个传输任务的并发控制状态。
pub(super) struct TaskControl {
    status: AtomicU8,
    intent: AtomicU8,
    generation: AtomicU64,
    changed: watch::Sender<u64>,
    transition_lock: Mutex<()>,
}

impl TaskControl {
    /// 创建指定初始状态的任务控制器。
    pub(super) fn new(status: u8) -> Self {
        let (changed, _) = watch::channel(0);
        Self {
            status: AtomicU8::new(status),
            intent: AtomicU8::new(CTL_NONE),
            generation: AtomicU64::new(0),
            changed,
            transition_lock: Mutex::new(()),
        }
    }

    /// 返回当前任务状态。
    pub(super) fn status(&self) -> u8 {
        self.status.load(Ordering::SeqCst)
    }

    /// 返回前端使用的状态名称。
    pub(super) fn status_str(&self) -> &'static str {
        status_str(self.status())
    }

    /// 返回当前执行代际。
    pub(super) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// 判断当前执行代际是否仍有效。
    pub(super) fn is_generation(&self, generation: u64) -> bool {
        self.generation() == generation
    }

    /// 订阅任务控制状态变化。
    pub(super) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// 请求暂停；等待中的任务立即暂停，活动任务在下一个检查点落地。
    pub(super) fn request_pause(&self) {
        let _transition = self.transition_lock.lock().unwrap();
        match self.status() {
            ST_PENDING => self.status.store(ST_PAUSED, Ordering::SeqCst),
            ST_RUNNING | ST_PACKING => self.intent.store(CTL_PAUSE, Ordering::SeqCst),
            _ => return,
        }
        self.notify();
    }

    /// 请求继续，并原子处理尚未落地或已经落地的暂停。
    pub(super) fn resume(&self) -> ResumeAction {
        let _transition = self.transition_lock.lock().unwrap();
        let status = self.status();
        if matches!(status, ST_RUNNING | ST_PACKING)
            && self.intent.load(Ordering::SeqCst) == CTL_PAUSE
        {
            self.intent.store(CTL_NONE, Ordering::SeqCst);
            self.notify();
            return ResumeAction::KeepRunning;
        }
        if status != ST_PAUSED {
            return ResumeAction::Ignored;
        }
        self.status.store(ST_PENDING, Ordering::SeqCst);
        self.intent.store(CTL_NONE, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.notify();
        ResumeAction::Requeue
    }

    /// 删除任务时取消当前执行体并切换执行代际。
    pub(super) fn cancel(&self) {
        let _transition = self.transition_lock.lock().unwrap();
        self.intent.store(CTL_CANCEL, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.status.store(ST_CANCELLED, Ordering::SeqCst);
        self.notify();
    }

    /// 会话断开时将活动任务转换为可重试失败状态。
    pub(super) fn interrupt(&self) -> bool {
        let _transition = self.transition_lock.lock().unwrap();
        if !matches!(self.status(), ST_PENDING | ST_RUNNING | ST_PACKING) {
            return false;
        }
        self.intent.store(CTL_CANCEL, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.status.store(ST_FAILED, Ordering::SeqCst);
        self.notify();
        true
    }

    /// 将失败任务切换到新执行代际并重新入队。
    pub(super) fn retry_failed(&self) -> bool {
        let _transition = self.transition_lock.lock().unwrap();
        if self.status() != ST_FAILED {
            return false;
        }
        self.status.store(ST_PENDING, Ordering::SeqCst);
        self.intent.store(CTL_NONE, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.notify();
        true
    }

    /// 在执行检查点落地暂停或取消请求，返回是否允许当前执行体继续。
    pub(super) fn checkpoint(&self, generation: u64) -> bool {
        let _transition = self.transition_lock.lock().unwrap();
        if !self.is_generation(generation) {
            return false;
        }
        match self.intent.load(Ordering::SeqCst) {
            CTL_PAUSE => {
                if matches!(self.status(), ST_RUNNING | ST_PACKING) {
                    self.status.store(ST_PAUSED, Ordering::SeqCst);
                }
                self.intent.store(CTL_NONE, Ordering::SeqCst);
                self.notify();
                false
            }
            CTL_CANCEL => false,
            _ => matches!(self.status(), ST_RUNNING | ST_PACKING),
        }
    }

    /// 判断活动执行体是否应停止等待当前异步操作。
    pub(super) fn stop_requested(&self, generation: u64) -> bool {
        !self.is_generation(generation)
            || self.intent.load(Ordering::SeqCst) != CTL_NONE
            || !matches!(self.status(), ST_RUNNING | ST_PACKING)
    }

    /// 判断当前执行体是否被取消或失效，但忽略待落地的暂停请求。
    pub(super) fn cancelled(&self, generation: u64) -> bool {
        !self.is_generation(generation)
            || self.intent.load(Ordering::SeqCst) == CTL_CANCEL
            || matches!(self.status(), ST_FAILED | ST_CANCELLED)
    }

    /// 将等待中的当前代际任务切换为运行中。
    pub(super) fn start(&self, generation: u64) -> bool {
        let _transition = self.transition_lock.lock().unwrap();
        if !self.is_generation(generation) || self.status() != ST_PENDING {
            return false;
        }
        self.status.store(ST_RUNNING, Ordering::SeqCst);
        true
    }

    /// 将运行中的当前代际任务切换为打包中。
    pub(super) fn begin_packing(&self, generation: u64) -> bool {
        let _transition = self.transition_lock.lock().unwrap();
        if !self.is_generation(generation) || self.status() != ST_RUNNING {
            return false;
        }
        match self.intent.load(Ordering::SeqCst) {
            CTL_PAUSE => {
                self.status.store(ST_PAUSED, Ordering::SeqCst);
                self.intent.store(CTL_NONE, Ordering::SeqCst);
                self.notify();
                return false;
            }
            CTL_CANCEL => return false,
            _ => {}
        }
        self.status.store(ST_PACKING, Ordering::SeqCst);
        true
    }

    /// 将打包中的当前代际任务恢复为运行中以便重试或下载。
    pub(super) fn finish_packing(&self, generation: u64) -> bool {
        let _transition = self.transition_lock.lock().unwrap();
        if !self.is_generation(generation) || self.status() != ST_PACKING {
            return false;
        }
        self.status.store(ST_RUNNING, Ordering::SeqCst);
        true
    }

    /// 将当前代际的活动任务标记为失败。
    pub(super) fn fail(&self, generation: u64) -> bool {
        let _transition = self.transition_lock.lock().unwrap();
        if !self.is_generation(generation) || !matches!(self.status(), ST_RUNNING | ST_PACKING) {
            return false;
        }
        match self.intent.load(Ordering::SeqCst) {
            CTL_PAUSE => {
                self.status.store(ST_PAUSED, Ordering::SeqCst);
                self.intent.store(CTL_NONE, Ordering::SeqCst);
                self.notify();
                return false;
            }
            CTL_CANCEL => return false,
            _ => {}
        }
        self.status.store(ST_FAILED, Ordering::SeqCst);
        self.notify();
        true
    }

    /// 将当前代际的运行中任务标记为完成。
    pub(super) fn complete(&self, generation: u64) -> bool {
        let _transition = self.transition_lock.lock().unwrap();
        if !self.is_generation(generation) || self.status() != ST_RUNNING {
            return false;
        }
        self.status.store(ST_COMPLETED, Ordering::SeqCst);
        self.notify();
        true
    }

    /// 更新聚合目录状态；聚合节点没有独立执行体。
    pub(super) fn set_aggregate_status(&self, status: u8) {
        let _transition = self.transition_lock.lock().unwrap();
        self.status.store(status, Ordering::SeqCst);
    }

    /// 判断指定代际任务是否仍在等待中。
    pub(super) fn is_pending_generation(&self, generation: u64) -> bool {
        self.is_generation(generation) && self.status() == ST_PENDING
    }

    /// 推送一次控制状态变化通知。
    fn notify(&self) {
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    #[cfg(test)]
    /// 测试中切换执行代际，用于验证旧执行体隔离。
    pub(super) fn set_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::SeqCst);
        self.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 继续先于暂停检查点时应撤销暂停并保持当前执行体运行。
    #[test]
    fn resume_before_checkpoint_keeps_runner_active() {
        let control = TaskControl::new(ST_RUNNING);
        control.request_pause();
        assert!(control.stop_requested(0));

        assert!(matches!(control.resume(), ResumeAction::KeepRunning));
        assert!(control.checkpoint(0));
        assert_eq!(control.status(), ST_RUNNING);
    }

    /// 暂停检查点先落地时，继续应切换代际并重新入队。
    #[test]
    fn resume_after_checkpoint_requeues_runner() {
        let control = TaskControl::new(ST_RUNNING);
        control.request_pause();
        assert!(!control.checkpoint(0));

        assert!(matches!(control.resume(), ResumeAction::Requeue));
        assert_eq!(control.status(), ST_PENDING);
        assert_eq!(control.generation(), 1);
    }

    /// 到达失败转换前收到的暂停请求仍应优先落地。
    #[test]
    fn pause_wins_over_failure_transition() {
        let control = TaskControl::new(ST_RUNNING);
        control.request_pause();

        assert!(!control.fail(0));
        assert_eq!(control.status(), ST_PAUSED);
    }
}
