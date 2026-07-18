Tauri命令

后端 见commands.rs 均返回Result<T,String> 注册于lib.rs invoke_handler 参数前端camelCase后端snake_case自动映射

命令分类 ssh_connect/disconnect terminal_open/write/resize path_is_dir(本地路径是否目录 供终端拖拽上传前拒绝文件夹) monitor_collect sftp_*(含create_archive/extract_archive) transfer_* 详细签名见api.ts(前端)与commands.rs(后端)

托管状态 SessionManager TransferManager通过Builder::manage注入 命令用State获取

插件 tauri-plugin-store/dialog/opener/clipboard-manager/single-instance(必须最先注册 再次启动回调unminimize+show+set_focus唤起已运行main窗口 仅Win/Mac/Linux) 权限见capabilities/default.json与editor.json 自绘标题栏需core:window窗口权限(minimize/toggle-maximize/unmaximize/close/start-dragging/is-maximized/internal-toggle-maximize/destroy) main创建/枚举编辑窗口需core:webview create-webview-window/get-all-webviews及window show/unminimize/set-focus 编辑窗口复用自绘TitleBar并按editor.json授权窗口控制 版本号getVersion已含core:default
