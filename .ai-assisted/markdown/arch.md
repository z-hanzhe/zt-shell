# 架构

## 职责

- ZTShell 是基于 Tauri 2 的跨平台桌面 SSH 客户端；前端使用 Vue 3、Pinia 与 xterm，后端使用 Rust、Tokio、russh 与 russh-sftp。
- 主窗口由监控面板、终端工作区和文件/传输工作区组成；终端区域占用剩余空间。
- `src-tauri/src/ssh/` 承载 SSH 领域能力，`commands.rs` 是 Tauri 命令边界，`src/` 承载界面、状态和调用封装。

## 入口与数据流

- 前端启动入口是 `src/main.ts`；主窗口根组件是 `App.vue`，文本编辑器通过窗口参数加载独立根组件。
- 前端经 `src/api.ts` 调用 Tauri 命令，命令层再协调 `SessionManager`、`TransferManager` 与 SSH 模块。
- 共享类型定义位于 `src/types.ts` 与 `src-tauri/src/ssh/types.rs`；修改跨端数据结构时应同步核对两处。

## 运行边界

- `SessionManager` 管理活动 SSH 会话及其派生的终端、SFTP、隧道和可中断文件操作；`TransferManager` 管理后台传输任务。
- 终端字节流通过 Tauri Channel 传递，关闭事件通过 `terminal://close//{sessionId}` 通知前端。
- 浏览器 Vite 页面仅用于布局预览；原生能力的调试限制见 `index.md` 全局陷阱速查表。
