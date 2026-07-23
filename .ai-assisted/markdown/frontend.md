前端结构

入口 App.vue自绘标题栏+主体+底部状态栏 主体flex三栏布局 分隔条可拖拽

组件 Icon.vue内置SVG图标 AppDialog.vue通用弹窗(confirmDanger红色/loading不可关但可配置警示操作 禁止点空白关闭 仅按钮或ESC关) MonitorPanel.vue TerminalPanel.vue选项卡栏(指针自绘拖拽排序因WebView2 HTML5拖放不稳定/滚动溢出/右键菜单) Terminal.vue封装xterm BottomPanel.vue(文件+传输选项卡) FileManager.vue TransferPanel.vue ConnectionManager.vue ConnectionEditor.vue SettingsDialog.vue TextEditorWindow.vue(monaco-editor独立窗口) TitleBar.vue

状态 connections.ts持久化连接与分组文件夹(folders树形嵌套/upsert/remove/upsertFolder/removeFolderRecursive递归删/reorderItems维护同级order/moveItems防环/duplicateConnection同级复制/countFolderContents) sessions.ts活动会话(open/close/activate/move/reconnect/markDisconnected/markActivity)(⚠ ref数组push后须find取代理元素改 勿直接改原始对象) settings.ts持久化设置(tauri-plugin-store load的options需含defaults字段) monitor.ts按会话采集(收发无关选项卡) transfers.ts传输任务 App.vue onMounted初始化

工具 utils.ts(formatBytes/genId/joinPath/parentPath) api.ts封装所有invoke types.ts前后端类型 composables/useEscClose全局回调栈栈顶弹窗响应ESC(天然支持嵌套)所有模态弹窗统一禁点空白关仅按钮/ESC关 hasOpenModal供FileManager的ESC避让

样式 styles.css全局CSS变量+scoped内联

标题栏 decorations:false data-tauri-drag-region拖拽双击最大化 设置按钮置于金刚键左侧

选项卡 指针自绘拖拽排序(WebView2 HTML5拖放不稳定) 溢出横向滚动按钮 左侧状态指示(点/叹号) 右键关闭/重连 重连复用同一sessionId原地重开通道保留xterm历史(reconnecting集合+Terminal suppressClose双重抑制旧通道terminal://close误标掉线) 掉线/远端exit经terminal://close由store.markDisconnected置disconnected并停监控 窗口关闭时含连接中的会话弹确认后destroy

终端 shallowRef防代理 allowProposedApi true(FitAddon/SearchAddon) Tokyo Night配色 背景不透明 右键菜单走原生剪贴板 快捷键attachCustomKeyEventHandler(Alt+Insert有选区时复制到原生剪贴板并立即写回终端/F3打开或继续查找) 菜单不显示快捷键文字，统一通过悬停提示说明各项快捷键，Ctrl+Insert与Shift+Insert不拦截 clear通过scrollOnEraseInDisplay将当前屏推入回滚区并拦主屏ESC[3J保留完整历史) 输入Promise队列保序 resize独立同步 隐写时doFit跳过 App层放行终端Ctrl组合与F3/F5/F7 FileManager的F5在事件源自.xterm时跳过 拖入单文件onDragDropEvent上传至终端当前目录(requestCwd 仅激活选项卡 多文件/文件夹经path_is_dir拒绝 同名走transferUpload的existNames弹AppDialog确认覆盖) Vite生产build.target保持es2021规避@xterm/xterm二次压缩缺陷(xterm.js#5800)

字体 @fontsource/cascadia-mono自托管 终端栈Consolas>Cascadia Mono UI中文系统栈

文件管理 左侧树+右侧列表 树聚焦后上下切换/左右与回车展开收起 列表表头排序/多选/框选/拖拽移动 右键菜单精简为编辑/重命名/新建/压缩解压/上传/下载/删除，键入快速定位两侧统一橙色命中态，列表按粘性表头下方的实际可视区域滚动定位 地址栏↑cd当前终端 ↓printf OSC标记读PWD后切文件路径 压缩/解压/批量删除加载弹窗以operationId请求中断并二次确认，迟到响应按ID隔离，中断后刷新真实目录且不承诺回滚 目录浏览状态按sessionId缓存(普通选项卡切换不请求SFTP/编辑与上传等外部变更标脏后切回刷新/终端断开或选项卡关闭清缓存/视图版本丢弃跨会话异步结果) 文本编辑以会话+路径哈希创建自绘标题栏独立Tauri窗口(复用TitleBar/可并行操作/重复打开聚焦已有窗口/选项卡关闭联动销毁/重连复用sessionId/ESC不关闭窗口) TextEditorWindow集成monaco-editor ESM worker 只读自动检测(sudo恒可写 普通exec test -w) 1MB/二进制确认 变更退出二次确认

监控面板 纯视图读monitor store 系统信息/CPU/内存/网卡图/磁盘/进程 未连骨架占位 采集出错底部提示 网卡历史按名切换 默认自动选网卡(物理优先 取历史累计流量最高者 有流量后自动锁定不再跳动 消失重新选 手动选后固定) 图双柱上传橙下载绿

连接管理器 ConnectionManager弹窗 文件夹树形分组(多级嵌套 folder/conn统一扁平行visibleRows 仅展开文件夹向下递归 同级按order显示/旧数据文件夹优先名称兜底)+主机/端口/用户名列 名称ellipsis+title悬停 行内单选 方向键导航 左右展开收起 回车连接或展开文件夹 双击文件夹展开/连接则连接 指针拖拽支持同父级前后排序写order与拖入文件夹/空白根目录移动 搜索态扁平化禁拖拽) 右键菜单连接/编辑/重命名/复制/悬停新建›[连接文件夹]/删除(新建位置按右键目标推断 复制仅连接 删除文件夹递归确认) ESC逐级关菜单→清选择→关窗 弹窗复用AppDialog 新建连接经editorParentId落入目标文件夹 后台FileManager经hasOpenModal避让方向键 ConnectionEditor左侧分组导航(连接配置承载现有表单 代理/隧道/更多预留) 私钥路径支持手输或原生文件选择回填
