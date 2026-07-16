架构总览

技术栈 前端Vue3+TypeScript+Vite+pinia+xterm 后端Rust+Tauri2+russh+russh-sftp+tokio

三栏布局 顶部标题栏底部状态栏 左监控 右上终端选项卡 右下文件/传输 分隔条可拖拽 终端区自适应填充剩余空间

会话模型 每会话对应一个后端SSH连接 sessionId前后端共用(前端genId生成 open时传给后端ssh_connect) 左监控与右下文件均以当前激活会话为数据源

数据流 前端invoke调用后端Tauri命令 SessionManager托管所有活动会话 终端输出通过事件异步推送

事件通道 终端数据terminal://data//{sessionId}(字节数组) 关闭terminal://close//{sessionId}

目录结构 src-tauri/src/ssh为SSH内核(types/session/manager/sftp/transfer/monitor) commands.rs命令层 src/components Vue组件 src/stores pinia状态 src/api.ts命令封装
