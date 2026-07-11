架构总览

技术栈 前端Vue3 TypeScript Vite pinia xterm 后端Rust Tauri2 russh russh-sftp tokio

整体布局 手写flex+自定义拖拽分隔条 顶部自绘标题栏底部状态栏 左侧监控面板固定像素宽约258 右下文件区固定像素高约300 窗口缩放时仅右上终端区自适应 左宽底高不变 分隔条可拖拽调整

会话模型 一个会话对应一个后端SSH连接 前端sessionId与后端sessionId共用同一字符串标识 前端genId生成 open时传给后端ssh_connect 左侧监控面板与右下文件管理器均以当前激活会话为数据源

数据流 前端通过src/api.ts封装的invoke调用后端Tauri命令 后端SessionManager作为Tauri托管状态集中管理所有活动会话 终端输出通过Tauri事件异步推送到前端

事件通道 终端数据事件名terminal://data//{sessionId}载荷为字节数组 终端关闭事件terminal://close//{sessionId}

目录结构 src-tauri/src/ssh为SSH内核 types类型 session单会话连接与终端 manager会话管理器 sftp文件操作 monitor监控采集 commands.rs为Tauri命令层 src/components为Vue组件 src/stores为pinia状态 src/api.ts命令封装 src/utils.ts工具函数 src/types.ts类型
