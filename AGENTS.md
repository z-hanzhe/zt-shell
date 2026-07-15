基于 Tauri2 + Rust 构建的跨平台桌面端 SSH 工具 ZTShell，项目将全程由 AI 开发，开发中需注意：

- 架构视野：开发时应具备项目整体架构视野，主动识别并抽离高复用模块（样式/工具函数/业务组件等等）
- 需求评估：收到需求后，先评估技术合理性及实现复杂度。不合理或复杂度高时，主动提示并给出替代方案或分步建议。
- 跨平台：Tauri 代码优先全平台兼容（Win/Mac/Linux/国产），至少 Win + Mac
- 三方库：需求实现前判断自己编写或第三方库。第三方库需申请并附理由，注意考虑跨平台兼容
- 规范维护：项目规范应自行维护至 AGENTS.md / CLAUDE.md 的 Prompt 节下便于 AI 工作（仅修改节后内容，节前为用户区），俩文件内容保持一致
- 文档索引：.ai-assisted/markdown/ 为功能文档目录，index.md 为入口。任务前先读索引定位按需加载，任务后按需更新文档与索引
- 格式约束：AGENTS.md / CLAUDE.md 的 Prompt 节及 .ai-assisted/markdown/ 下文档禁用无意义的格式缩进换行，压缩表述，控制文档长度禁止只增不减，修改前先审查现有内容，合并同类、移除过期信息，另外 Prompt 节前已存在的规范不要在 Prompt 下重复声明，防止提示词膨胀

# Prompt

项目技术栈 前端Vue3 TypeScript Vite pinia xterm 后端Rust Tauri2 russh russh-sftp tokio

已引入第三方库及理由 russh纯Rust SSH客户端跨平台免OpenSSL 用ring后端替代aws-lc-rs避免Windows需NASM default-features=false features=ring flate2 rsa russh-sftp配套SFTP tokio异步运行时 dashmap并发安全会话存储 uuid标识 anyhow错误处理 @xterm/xterm终端渲染VSCode同款 @xterm/addon-fit终端尺寸自适应 @xterm/addon-search终端查找高亮官方插件 tauri-plugin-clipboard-manager原生剪贴板读写避免WebView权限询问弹窗 手写flex固定尺寸布局 pinia状态管理 @tauri-apps/plugin-store配置持久化 @tauri-apps/plugin-dialog本地文件对话框 @fontsource/cascadia-mono内置终端等宽字体自托管避免跨平台字库缺失 monaco-editor远程文本编辑器VSCode内核支持多语言高亮与Vite ESM worker

功能文档 见.ai-assisted/markdown/index.md 任务前先读索引 涉及模块加载对应文档 架构arch SSH内核ssh-core 文件管理sftp 传输任务transfer 监控monitor 前端frontend 命令commands

关键约定 前端sessionId与后端sessionId共用同一标识由前端genId生成 Tauri命令前端invoke传camelCase键后端snake_case参数 ConnectionConfig等结构体serde用rename_all camelCase 终端事件terminal://data//{sessionId}与terminal://close//{sessionId} tauri-plugin-store的load的options需含defaults字段

监控限制 仅支持Linux远端 依赖标准/proc与coreutils df ps

文件管理sudo提权 会话可切普通/sudo两种SFTP sudo走专用通道exec sudo -S复用登录密码 提示走stderr而russh into_stream只读stdout故不污染二进制协议 无需PTY 仅Linux 详见sftp.md

新增命令流程 在ssh模块实现能力 commands.rs加tauri命令 lib.rs注册handler src/api.ts加封装 涉及权限更新capabilities/default.json

界面主题 参考FinalShell 4.6.5 采用浅色主题 面板浅灰白 仅终端深色用Tokyo Night配色 见finalshell-ref.html于.ai-ignore

调试须知 SSH等invoke能力仅在Tauri窗口内可用 浏览器localhost:1420仅供预览布局 invoke会因无Tauri运行时而失败 需npm run tauri dev在原生窗口测试连接

Vue响应式坑 pinia的ref数组中push普通对象后 勿直接改原始对象引用 需通过sessions.value.find取代理元素再改 否则视图不刷新 曾致卡在正在连接

自绘标题栏 tauri.conf设decorations:false禁用系统标题栏 拖拽与双击最大化用原生data-tauri-drag-region属性 勿手动mousedown+startDragging会吞掉双击导致双击无法切换最大化 需core:window的minimize/toggle-maximize/unmaximize/close/start-dragging/is-maximized/internal-toggle-maximize权限

终端配色 Tokyo Night 背景#1a1b26前景#c0caf5 完整16色ANSI调色板使ls --color等转义色正确高亮 见Terminal.vue的theme 背景用不透明纯色勿allowTransparency否则WebView2渲染成纯黑 字体栈Consolas优先再回落内置Cascadia Mono
