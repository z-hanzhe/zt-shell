SSH内核

依赖 russh 0.55 纯Rust实现 无需OpenSSL 跨平台 russh-sftp 2 SFTP子系统 tokio异步运行时 crypto后端用ring替代默认aws-lc-rs 因aws-lc-rs在Windows下构建依赖NASM 用default-features=false features=ring flate2 rsa 保留RSA密钥支持

连接认证 见session.rs SshSession::connect 支持密码认证authenticate_password与私钥认证authenticate_publickey 私钥用load_secret_key加载 RSA哈希用best_supported_rsa_hash协商 check_server_key当前信任所有服务端公钥 后续可扩展known_hosts校验

终端 open_terminal开启session通道 request_pty申请xterm-256color伪终端 request_shell启动shell 参数on_data为tauri::ipc::Channel<Response> 用channel.split()拆读写两半分别spawn独立tokio任务 读任务持续消费wait输出并用Response::new发送Raw字节 Channel保证前端按序处理 写任务处理Write/Resize(window_change)/Close指令 关闭事件仍走app.emit(terminal://close//) 前端Channel失效后停止输出任务 写任务退出时close通道使读任务退出

一次性命令 exec_command开新通道exec执行命令收集输出 用于监控采集

会话管理器 见manager.rs SessionManager用dashmap存储SessionEntry 每条含SSH会话 终端控制发送端 惰性SFTP会话 提供connect disconnect open_terminal write_terminal resize_terminal exec sftp等方法

注意点 russh的ChannelMsg::Data载荷为CryptoVec 用to_vec或切片转换 认证结果需检查success 终端通道读写分离于两个任务勿合并到单select Channel<Vec<u8>>会按Serialize发送JSON数组 要发送ArrayBuffer必须使用Channel<Response>并send(Response::new(bytes))
