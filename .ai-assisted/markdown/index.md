# 功能文档索引

## ⚠️ 全局陷阱速查表

| 模块 | 陷阱 | 影响范围 |
| --- | --- | --- |
| 会话 | 前端生成的 `sessionId` 是前后端共同标识；重连必须复用它，否则会话相关状态无法延续。 | SSH、监控、文件管理、传输、文本编辑 |
| 主机密钥 | 用户确认后必须使用所展示的完整公钥约束下一次握手；只传“已确认”状态会失去确认与重连之间的身份绑定。 | SSH、Tauri 调用、会话 |
| Tauri 调用 | 前端调用参数使用 camelCase，Rust 命令参数使用 snake_case，由 Serde 自动映射。 | `api.ts`、`commands.rs`、类型定义 |
| 调试 | 浏览器中的 Vite 页面不能调用 Tauri `invoke`；验证 SSH、文件和窗口能力须运行 `npm run tauri dev`。 | 所有原生能力 |

## 模块索引

| 文档 | 职责 | 主要入口 |
| --- | --- | --- |
| [架构](arch.md) | 应用分层、数据流与运行边界 | `App.vue`、`lib.rs` |
| [SSH 内核](ssh-core.md) | 建连、终端、代理、隧道和会话生命周期 | `ssh/manager.rs`、`ssh/session.rs` |
| [SFTP](sftp.md) | 远端文件操作、sudo 文件会话和可中断操作 | `ssh/sftp.rs`、`FileManager.vue` |
| [传输](transfer.md) | 后台上传下载、断点续传和任务控制 | `ssh/transfer.rs`、`TransferPanel.vue` |
| [监控](monitor.md) | Linux 远端指标采集与按会话展示 | `ssh/monitor.rs`、`monitor.ts` |
| [前端](frontend.md) | Vue 组件、Pinia 状态和独立编辑器窗口 | `main.ts`、`src/components/` |
| [Tauri 命令](commands.md) | 前后端命令边界、托管状态和权限 | `commands.rs`、`api.ts` |
| [发布](release.md) | 多平台构建和发布草稿流程 | `.github/workflows/release.yml` |
