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
