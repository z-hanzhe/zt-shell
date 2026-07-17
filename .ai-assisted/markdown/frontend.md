前端结构

入口 App.vue自绘标题栏+主体+底部状态栏 主体flex三栏布局 分隔条可拖拽

组件 Icon.vue内置SVG图标 AppDialog.vue通用弹窗(confirmDanger红色/loading不可关 禁止点空白关闭 仅按钮或ESC关) MonitorPanel.vue TerminalPanel.vue选项卡栏(指针自绘拖拽排序因WebView2 HTML5拖放不稳定/滚动溢出/右键菜单) Terminal.vue封装xterm BottomPanel.vue(文件+传输选项卡) FileManager.vue TransferPanel.vue ConnectionManager.vue ConnectionEditor.vue SettingsDialog.vue TextEditorDialog.vue(monaco-editor) TitleBar.vue

状态 connections.ts持久化连接 sessions.ts活动会话(open/close/activate/move/reconnect/markDisconnected/markActivity) settings.ts持久化设置 monitor.ts按会话采集(收发无关选项卡) transfers.ts传输任务 App.vue onMounted初始化

工具 utils.ts(formatBytes/genId/joinPath/parentPath) api.ts封装所有invoke types.ts前后端类型 composables/useEscClose全局回调栈栈顶弹窗响应ESC(天然支持嵌套)所有模态弹窗统一禁点空白关仅按钮/ESC关 hasOpenModal供FileManager的ESC避让

样式 styles.css全局CSS变量+scoped内联

标题栏 decorations:false data-tauri-drag-region拖拽双击最大化 设置按钮置于金刚键左侧

选项卡 指针自绘拖拽排序(WebView2 HTML5拖放不稳定) 溢出横向滚动按钮 左侧状态指示(点/叹号) 右键关闭/重连 重连复用同一sessionId原地重开通道保留xterm历史(reconnecting集合+Terminal suppressClose双重抑制旧通道terminal://close误标掉线) 掉线/远端exit经terminal://close由store.markDisconnected置disconnected并停监控 窗口关闭时含连接中的会话弹确认后destroy

终端 shallowRef防代理 allowProposedApi true(FitAddon/SearchAddon) Tokyo Night配色 背景不透明 右键菜单走原生剪贴板 快捷键attachCustomKeyEventHandler(clear仅拦主屏ESC[3J保留历史) 输入Promise队列保序 resize独立同步 隐写时doFit跳过 App层放行终端Ctrl组合与F3/F5/F7 FileManager的F5在事件源自.xterm时跳过 拖入单文件onDragDropEvent上传至终端当前目录(requestCwd 仅激活选项卡 多文件/文件夹经path_is_dir拒绝 同名走transferUpload的existNames弹AppDialog确认覆盖)

字体 @fontsource/cascadia-mono自托管 终端栈Consolas>Cascadia Mono UI中文系统栈

文件管理 左侧树+右侧列表 表头排序 多选/框选/拖拽移动 右键上传下载打包下载删除重命名 键入快速定位 地址栏↑cd当前终端 ↓printf OSC标记读PWD后切文件路径 TextEditorDialog集成monaco-editor ESM worker 只读自动检测(sudo恒可写 普通exec test -w) 1MB/二进制确认 变更退出二次确认

监控面板 纯视图读monitor store 系统信息/CPU/内存/网卡图/磁盘/进程 未连骨架占位 采集出错底部提示 网卡历史按名切换 默认自动选网卡(物理优先 取历史累计流量最高者 有流量后自动锁定不再跳动 消失重新选 手动选后固定) 图双柱上传橙下载绿
