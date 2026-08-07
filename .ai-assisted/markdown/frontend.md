# 前端

## 入口与组件边界

- `main.ts` 根据窗口参数加载 `App.vue` 或 `TextEditorWindow.vue`；两者各自创建 Vue 与 Pinia 应用。
- `App.vue` 负责主窗口布局、应用初始化、窗口关闭协调和连接管理器入口。
- `TerminalPanel.vue` 与 `Terminal.vue` 承载会话选项卡和 xterm；`BottomPanel.vue` 在文件管理与传输面板之间切换。
- `ConnectionManager.vue`、`ConnectionEditor.vue`、`ProxySettings.vue` 和 `TunnelSettings.vue` 管理已保存连接及其扩展配置。
- `TextEditorWindow.vue` 是单例独立工作区，使用 Monaco 编辑远端文件；`editorWindows.ts` 负责主窗口与编辑器窗口通信。

## 状态与调用边界

| 模块 | 职责 |
| --- | --- |
| `sessions.ts` | 活动 SSH 会话、选项卡状态和重连。 |
| `connections.ts`、`proxies.ts` | 持久化连接、分组文件夹和共享代理。 |
| `monitor.ts` | 按会话采集、缓存监控数据和网卡历史。 |
| `transfers.ts` | 监听传输事件并维护任务快照。 |
| `settings.ts` | 持久化终端和监控偏好。 |

- `api.ts` 是所有 Tauri 调用的唯一封装入口，`types.ts` 是前端跨端类型入口。
- `utils.ts` 放置路径、标识和展示格式等通用工具；组件间共享行为优先放入可复用模块而非复制。

## 稳定约束

- 所有模态界面的 Escape 行为通过 `composables/useEscClose.ts` 协调，新增模态界面必须接入该栈。
- 文件管理按会话保留浏览状态；异步结果必须确认仍属于当前会话与视图，外部文件变更后应标记并在切回时刷新。
- 关闭 SSH 会话时必须同步关闭编辑器工作区中属于该会话的文档；编辑器中的同一会话和路径只应对应一个文档。
- ⚠️ 陷阱：xterm 的搜索装饰依赖 proposed API，初始化终端时必须保持 `allowProposedApi`；Vite 构建目标不得低于 `es2021`，否则 xterm 生产构建会异常。
