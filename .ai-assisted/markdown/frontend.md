# 前端

## 入口与组件边界

- `main.ts` 根据窗口参数加载 `App.vue` 或 `TextEditorWindow.vue`；两者各自创建 Vue 与 Pinia 应用。
- `App.vue` 负责主窗口布局、应用初始化、窗口关闭协调和连接管理器入口。
- `TerminalPanel.vue` 与 `Terminal.vue` 承载会话选项卡和 xterm；`BottomPanel.vue` 在文件管理与传输面板之间切换。
- `HostKeyDialog.vue` 展示首次主机信任与密钥变化警告，由应用根组件协调等待确认的会话。
- `ConnectionManager.vue`、`ConnectionEditor.vue`、`ProxySettings.vue` 和 `TunnelSettings.vue` 管理已保存连接及其扩展配置。
- `TextEditorWindow.vue` 是单例独立工作区，使用 Monaco 编辑远端文件；`editorWindows.ts` 负责主窗口与编辑器窗口通信。

## 状态与调用边界

| 模块 | 职责 |
| --- | --- |
| `sessions.ts` | 活动 SSH 会话、主机密钥确认状态、选项卡状态和重连。 |
| `connections.ts`、`proxies.ts` | 持久化连接元数据、分组文件夹、展开状态和共享代理；秘密凭据单独存入系统凭据库。 |
| `monitor.ts` | 按会话采集、缓存监控数据和网卡历史。 |
| `transfers.ts` | 监听传输事件并维护任务快照。 |
| `settings.ts` | 持久化终端和监控偏好。 |

- `api.ts` 是所有 Tauri 调用的唯一封装入口，`types.ts` 是前端跨端类型入口。
- `connectionTransfer.ts` 是连接导入导出 V1 文件契约、层级重建、名称冲突和代理复用规则的唯一数据边界。
- `utils.ts` 放置路径、标识和展示格式等通用工具；组件间共享行为优先放入可复用模块而非复制。

## 稳定约束

- 所有模态界面的 Escape 行为通过 `composables/useEscClose.ts` 协调，新增模态界面必须接入该栈。
- 连接、私钥和代理密码不得写入插件存储或回读到编辑器；已保存凭据只保留可用性标记并由 Rust 按稳定连接标识从系统凭据库读取。
- 导出文件会包含明文凭据但不包含私钥文件；所有导出入口必须先明确警告，私钥路径跨设备导入后由用户自行确认可用性。
- 导入连接时优先复用同一来源的原文件夹，否则复用目标父级下的同名文件夹；同层连接重名时追加递增序号。
- 会话建连异步返回时须按 `sessionId` 重新定位 store 内对象；直接比较或修改入数组前的原始对象会绕过 Vue 响应式代理并丢失状态更新。
- 同一 `sessionId` 的重连必须单次执行；并发建连会使前后端接受不同批次的异步结果。
- 文件管理按会话保留浏览状态；异步结果必须确认仍属于当前会话与视图，外部文件变更后应标记并在切回时刷新。
- 关闭 SSH 会话时必须同步关闭编辑器工作区中属于该会话的文档；编辑器中的同一会话和路径只应对应一个文档。
- ⚠️ 陷阱：xterm 的搜索装饰依赖 proposed API，初始化终端时必须保持 `allowProposedApi`；Vite 构建目标不得低于 `es2021`，否则 xterm 生产构建会异常。
