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
