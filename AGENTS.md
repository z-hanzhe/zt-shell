基于 Tauri2 + Rust 构建的跨平台桌面端 SSH 工具 ZTShell，项目将全程由 AI 开发，开发中需注意：

- 架构视野：开发时应具备项目整体架构视野，主动识别并抽离高复用模块（样式/工具函数/业务组件等等）
- 需求评估：收到需求后，先评估技术合理性及实现复杂度。不合理或复杂度高时，主动提示并给出替代方案或分步建议。
- 跨平台：Tauri 代码优先全平台兼容（Win/Mac/Linux/国产），至少 Win + Mac
- 三方库：需求实现前判断自己编写或第三方库。第三方库需申请并附理由，注意考虑跨平台兼容
- 规范维护：项目规范应自行维护至 AGENTS.md / CLAUDE.md 的 Prompt 节下便于 AI 工作（仅修改节后内容，节前为用户区），俩文件内容保持一致
- 文档索引：.ai-assisted/markdown/ 为功能文档目录，index.md 为入口。任务前先读索引定位，任务后更新索引，模块文档只记录职责、入口函数、数据流及特殊陷阱，不记录变量名/像素值/事件绑定等等实现细节，可参考现有文档
- 格式约束：AGENTS.md / CLAUDE.md 的 Prompt 节及 .ai-assisted/markdown/ 下文档禁用无意义的格式缩进换行，压缩表述，控制文档长度禁止只增不减，修改前先审查现有内容，合并同类、移除过期信息，另外 Prompt 节前已存在的规范不要在 Prompt 下重复声明，防止提示词膨胀

# Prompt

技术栈 前端Vue3+TypeScript+Vite+pinia+xterm 后端Rust+Tauri2+russh+russh-sftp+tokio

第三方库选型理由 见Cargo.toml与package.json 关键: russh纯Rust SSH免OpenSSL跨平台 ring替代aws-lc-rs避Windows需NASM @xterm/xterm终端渲染 tauri-plugin-store/dialog/opener/clipboard-manager均为官方插件 single-instance必须最先注册且再次启动时回调唤起已运行窗口 monaco-editor远程文本编辑 @fontsource/cascadia-mono内置终端等宽字体自托管跨平台一致

功能文档 .ai-assisted/markdown/index.md 任务前读索引定位模块 涉及模块加载对应文档

关键约定 前端sessionId与后端共用同一标识由前端genId生成 Tauri命令前端invoke传camelCase键后端snake_case参数 ConnectionConfig等结构体serde rename_all camelCase 终端输出走ipc::Channel<ArrayBuffer>关闭事件terminal://close//{sessionId} tauri-plugin-store的load的options需含defaults字段

监控限制 仅Linux远端 依赖/proc与coreutils df ps

SFTP sudo提权 会话可切普通/sudo两种 sudo走exec sudo -S复用登录密码 提示走stderr不污染stdout 无PTY 仅Linux 详见sftp.md

新增命令流程 ssh模块实现能力 commands.rs加tauri命令 lib.rs注册handler src/api.ts加封装 涉及权限更新capabilities/default.json

界面主题 参考FinalShell 4.6.5 浅色 仅终端深色Tokyo Night配色 见finalshell-ref.html于.ai-ignore

调试须知 invoke仅Tauri窗口内可用 浏览器localhost:1420仅预览布局 需npm run tauri dev

Vue响应式坑 pinia ref数组push普通对象后勿直接改原始对象引用 需通过sessions.value.find取代理元素再改

自绘标题栏 decorations:false禁用系统标题栏 拖拽与双击最大化用原生data-tauri-drag-region 勿手动mousedown+startDragging

终端配色 Tokyo Night 背景#1a1b26前景#c0caf5 完整16色ANSI 不透明背景(勿allowTransparency否则WebView2渲染纯黑) 字体Consolas>Cascadia Mono

终端构建 Vite build.target es2021 规避@xterm/xterm压缩缺陷(xterm.js#5800)

文本编辑 使用复用TitleBar的自绘标题栏独立Tauri WebviewWindow 会话+远端路径唯一定位 重复打开聚焦已有窗口 ESC不关闭窗口 关闭会话选项卡须销毁所属编辑窗口 重连复用sessionId使窗口继续读写

会话生命周期 终端channel是会话核心 结束后后端须按条目身份清理同代SSH/SFTP与传输资源 防止旧channel关闭误删重连后的同sessionId新会话

文件管理会话状态 目录列表/路径/树/sudo按sessionId缓存 普通选项卡切换不刷新SFTP 外部变更标脏后切回刷新 终端断开或选项卡关闭清缓存 异步结果须校验视图版本防跨会话污染

远端压缩解压 普通模式exec调用zip/tar/unzip sudo SFTP提权不作用于exec故sudo模式禁止 路径必须shell转义并校验单层条目名

终端Alt+Insert 仅有选区时复制到原生剪贴板并立即写回终端 无选区放行
