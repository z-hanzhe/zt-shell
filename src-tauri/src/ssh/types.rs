//! SSH 连接配置与会话相关的数据类型定义

use serde::{Deserialize, Serialize};

/// 认证方式
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AuthType {
    /// 密码认证
    Password,
    /// 私钥认证
    PrivateKey,
}

/// 代理协议
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ProxyType {
    /// SOCKS4，本地解析目标 IPv4
    Socks4,
    /// SOCKS4A，由代理解析目标域名
    Socks4a,
    /// SOCKS5
    Socks5,
    /// HTTP 1.1 CONNECT
    Http,
}

/// 隧道类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum TunnelType {
    /// 本地拨出：监听本机端口，连接进入后由服务器侧拨向目标
    Local,
    /// 远程传入：监听服务器端口，连接进入后回到客户端侧目标
    Remote,
    /// 动态 SOCKS4/5：监听本机端口，按客户端请求动态拨出
    Dynamic,
    /// 动态 HTTP：监听本机 HTTP 代理端口，按客户端请求动态拨出
    DynamicHttp,
}

/// SSH 建连使用的代理配置快照
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    /// 代理唯一标识
    pub id: String,
    /// 代理显示名称
    pub name: String,
    /// 代理协议
    pub proxy_type: ProxyType,
    /// 代理服务器地址
    pub host: String,
    /// 代理服务器端口
    pub port: u16,
    /// SOCKS4 用户标识或 SOCKS5/HTTP 用户名
    #[serde(default)]
    pub username: Option<String>,
    /// SOCKS5 密码或 HTTP Basic 密码
    #[serde(default)]
    pub password: Option<String>,
}

fn default_true() -> bool {
    true
}

/// SSH 隧道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfig {
    /// 隧道唯一标识
    pub id: String,
    /// 隧道显示名称
    #[serde(default)]
    pub name: String,
    /// 隧道类型
    pub tunnel_type: TunnelType,
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
    /// 是否仅接受监听端本机连接
    #[serde(default = "default_true")]
    pub local_only: bool,
    /// 监听端口
    pub listen_port: u16,
    /// 目标主机，本地/远程隧道必填
    #[serde(default)]
    pub target_host: Option<String>,
    /// 目标端口，本地/远程隧道必填
    #[serde(default)]
    pub target_port: Option<u16>,
}

/// 前端传入的连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    /// 连接的唯一标识
    pub id: String,
    /// 连接显示名称
    pub name: String,
    /// 主机地址
    pub host: String,
    /// 端口
    pub port: u16,
    /// 用户名
    pub username: String,
    /// 认证方式
    pub auth_type: AuthType,
    /// 密码（密码认证时使用）
    #[serde(default)]
    pub password: Option<String>,
    /// 私钥文件路径（私钥认证时使用）
    #[serde(default)]
    pub private_key_path: Option<String>,
    /// 私钥口令（私钥有加密时使用）
    #[serde(default)]
    pub passphrase: Option<String>,
    /// 当前连接使用的代理配置快照，空表示直连
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    /// 用户备注，后端建连暂不使用
    #[serde(default)]
    pub remark: Option<String>,
    /// 当前连接的隧道列表，运行时每个会话独立启动
    #[serde(default)]
    pub tunnels: Vec<TunnelConfig>,
}

/// SSH 建连结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    /// 会话标识
    pub session_id: String,
    /// 隧道启动警告，非空表示 SSH 已连接但部分隧道未启用
    pub tunnel_warnings: Vec<String>,
}

/// 一条 SFTP 文件条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// 文件名
    pub name: String,
    /// 是否为目录
    pub is_dir: bool,
    /// 是否为符号链接
    pub is_symlink: bool,
    /// 文件大小（字节）
    pub size: u64,
    /// 权限位（如 0o755）
    pub permissions: u32,
    /// 权限字符串（如 drwxr-xr-x）
    pub permissions_str: String,
    /// 修改时间（Unix 秒级时间戳）
    pub modified: u64,
    /// 属主用户
    pub owner: String,
    /// 属组
    pub group: String,
}
