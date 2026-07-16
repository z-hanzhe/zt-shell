SSH内核

连接认证 支持密码与私钥两种 私钥用load_secret_key加载 RSA哈希协商 check_server_key当前信任所有服务端公钥(后续可扩展known_hosts校验)

终端 open_terminal申请PTY+xterm-256color后启动shell 输出经Channel<Response>发送Raw字节(不可用Channel<Vec<u8>>否则JSON数组) 关闭事件走app.emit(terminal://close//) 读写分离于两个tokio任务

一次性命令 exec_command开exec通道收集stdout 用于监控等场景

会话管理器 SessionManager用dashmap存储 每条含SSH会话+终端控制发端+惰性SFTP
